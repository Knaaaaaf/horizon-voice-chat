#!/usr/bin/env python3
"""Authenticated HTTPS relay for Horizon Voice Chat.

Requires a trusted TLS certificate and an explicit client/token allow-list.
Audio is held in memory only; this is a room relay, not yet a proximity-aware
server plugin. Do not put this endpoint on the public Internet without TLS.
"""
from __future__ import annotations

from collections import deque
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hmac
import json
import os
import re
import ssl
import sys
import threading
import time
from typing import Any

MAGIC = b"HVC1"
VERSION = 1
HEADER_LEN = 14
MAX_PACKET_BYTES = 1200
MAX_BODY_BYTES = 64 * 1024
MAX_UPLOAD_FRAMES = 32
MAX_RESPONSE_FRAMES = 256
MAX_RESPONSE_BYTES = 512 * 1024
ROOM_HISTORY_FRAMES = 2048
ROOM_HISTORY_SECONDS = 4.0
MAX_ROOM_CLIENTS = 16
MAX_PACKETS_PER_SECOND = 125
MAX_BYTES_PER_SECOND = 128_000
MAX_GETS_PER_SECOND = 40
ID_RE = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
TOKEN_RE = re.compile(r"^[A-Za-z0-9_-]{32,256}$")
SESSION_RE = re.compile(r"^[A-Za-z0-9._-]{1,96}$")


@dataclass(frozen=True)
class ClientCredential:
    token: str
    rooms: frozenset[str]


@dataclass(frozen=True)
class FrameRecord:
    cursor: int
    received_at: float
    slot: int
    sender: str
    packet: bytes


@dataclass
class Room:
    cursor: int = 0
    frames: deque[FrameRecord] = field(
        default_factory=lambda: deque(maxlen=ROOM_HISTORY_FRAMES)
    )
    members: dict[str, float] = field(default_factory=dict)


@dataclass
class Traffic:
    uploads: deque[tuple[float, int, int]] = field(default_factory=deque)
    gets: deque[float] = field(default_factory=deque)


_LOCK = threading.RLock()
_ROOMS: dict[str, Room] = {}
_TRAFFIC: dict[str, Traffic] = {}
_SEQUENCES: dict[tuple[str, str], tuple[int, float]] = {}


def safe_id(value: str) -> bool:
    return bool(ID_RE.fullmatch(value))


def parse_clients(raw: str) -> dict[str, ClientCredential]:
    """Validate HVC_RELAY_CLIENTS JSON without ever logging token values."""
    value = json.loads(raw)
    if not isinstance(value, dict) or not value or len(value) > 256:
        raise ValueError("HVC_RELAY_CLIENTS must be a JSON object with 1–256 clients")
    clients: dict[str, ClientCredential] = {}
    seen_tokens: list[str] = []
    for client_id, item in value.items():
        if not isinstance(client_id, str) or not safe_id(client_id):
            raise ValueError("client IDs must use 1–64 ASCII letters, digits, '.', '_' or '-'")
        if not isinstance(item, dict):
            raise ValueError(f"credential entry for {client_id!r} must be an object")
        token = item.get("token")
        rooms = item.get("rooms")
        if not isinstance(token, str) or not TOKEN_RE.fullmatch(token):
            raise ValueError(f"token for {client_id!r} must be at least 32 non-whitespace characters")
        if any(hmac.compare_digest(token, previous) for previous in seen_tokens):
            raise ValueError("each configured client must have a unique relay token")
        if (
            not isinstance(rooms, list)
            or not rooms
            or len(rooms) > 64
            or len(set(rooms)) != len(rooms)
            or any(not isinstance(room, str) or not safe_id(room) for room in rooms)
        ):
            raise ValueError(f"rooms for {client_id!r} must be a list of valid room IDs")
        seen_tokens.append(token)
        clients[client_id] = ClientCredential(token, frozenset(rooms))
    return clients


def is_newer_sequence(new: int, previous: int) -> bool:
    """u32 wrap-safe sequence comparison; duplicates and stale packets fail."""
    difference = (new - previous) & 0xFFFFFFFF
    return 0 < difference < 0x80000000


def validate_audio_packet(packet: bytes) -> tuple[int, int]:
    """Validate the complete v1 packet and PCM16 payload; return seq and length."""
    if len(packet) < HEADER_LEN or len(packet) > MAX_PACKET_BYTES:
        raise ValueError("packet size outside limit")
    if packet[:4] != MAGIC or packet[4] != VERSION or packet[5] != 2:
        raise ValueError("not a v1 audio packet")
    if packet[6:8] != b"\0\0":
        raise ValueError("reserved header flags must be zero")
    sequence = int.from_bytes(packet[8:12], "little")
    payload_len = int.from_bytes(packet[12:14], "little")
    if payload_len > MAX_PACKET_BYTES - HEADER_LEN or payload_len != len(packet) - HEADER_LEN:
        raise ValueError("payload length mismatch")
    payload = packet[HEADER_LEN:]
    if len(payload) < 6:
        raise ValueError("PCM header is too short")
    sample_rate = int.from_bytes(payload[:4], "little")
    sample_count = int.from_bytes(payload[4:6], "little")
    if not 8_000 <= sample_rate <= 48_000:
        raise ValueError("sample rate outside allowed range")
    if sample_count != (sample_rate + 99) // 100:
        raise ValueError("audio must contain one approximately 10 ms frame")
    if len(payload) != 6 + sample_count * 2:
        raise ValueError("PCM sample count does not match payload length")
    return sequence, len(packet)


def decode_upload(body: bytes, previous_sequence: int | None = None) -> tuple[list[bytes], int | None]:
    """Decode HVB1 length-prefixed frames. Reject the whole batch on any error."""
    if len(body) < 6 or len(body) > MAX_BODY_BYTES or body[:4] != b"HVB1":
        raise ValueError("invalid upload envelope")
    count = int.from_bytes(body[4:6], "little")
    if not 1 <= count <= MAX_UPLOAD_FRAMES:
        raise ValueError("upload frame count outside limit")
    cursor = 6
    frames: list[bytes] = []
    last_sequence = previous_sequence
    for _ in range(count):
        if cursor + 2 > len(body):
            raise ValueError("truncated upload length")
        size = int.from_bytes(body[cursor:cursor + 2], "little")
        cursor += 2
        if not HEADER_LEN <= size <= MAX_PACKET_BYTES or cursor + size > len(body):
            raise ValueError("upload frame size outside limit")
        packet = body[cursor:cursor + size]
        sequence, _ = validate_audio_packet(packet)
        if last_sequence is not None and not is_newer_sequence(sequence, last_sequence):
            raise ValueError("duplicate or stale audio sequence")
        last_sequence = sequence
        frames.append(packet)
        cursor += size
    if cursor != len(body):
        raise ValueError("trailing bytes in upload")
    return frames, last_sequence


def encode_download(
    cursor: int, frames: list[FrameRecord], own_id: str, server_slot: int
) -> bytes:
    """Encode HVR1 with cursor, server time-slot and bounded remote packets."""
    selected: list[FrameRecord] = []
    total = 22
    scan_cursor = cursor
    exhausted = True
    for frame in frames:
        if frame.cursor <= cursor:
            continue
        if frame.sender == own_id:
            scan_cursor = frame.cursor
            continue
        cost = 8 + 1 + len(frame.sender.encode("ascii")) + 2 + len(frame.packet)
        if len(selected) >= MAX_RESPONSE_FRAMES or total + cost > MAX_RESPONSE_BYTES:
            exhausted = False
            break
        selected.append(frame)
        total += cost
        scan_cursor = frame.cursor
    if exhausted and frames:
        scan_cursor = max(scan_cursor, frames[-1].cursor)
    if not frames:
        scan_cursor = cursor
    body = bytearray(b"HVR1")
    body.extend(scan_cursor.to_bytes(8, "little"))
    body.extend(server_slot.to_bytes(8, "little"))
    body.extend(len(selected).to_bytes(2, "little"))
    for frame in selected:
        sender = frame.sender.encode("ascii")
        body.extend(frame.slot.to_bytes(8, "little"))
        body.append(len(sender))
        body.extend(sender)
        body.extend(len(frame.packet).to_bytes(2, "little"))
        body.extend(frame.packet)
    return bytes(body)


def _prune_room(room: Room, now: float) -> None:
    while room.frames and (
        now - room.frames[0].received_at > ROOM_HISTORY_SECONDS
        or len(room.frames) > ROOM_HISTORY_FRAMES
    ):
        room.frames.popleft()
    room.members = {
        client: seen for client, seen in room.members.items() if now - seen < 30.0
    }


def _allow_upload(client_id: str, packet_count: int, byte_count: int, now: float) -> bool:
    traffic = _TRAFFIC.setdefault(client_id, Traffic())
    while traffic.uploads and now - traffic.uploads[0][0] >= 1.0:
        traffic.uploads.popleft()
    packets = sum(item[1] for item in traffic.uploads)
    size = sum(item[2] for item in traffic.uploads)
    if packets + packet_count > MAX_PACKETS_PER_SECOND:
        return False
    if size + byte_count > MAX_BYTES_PER_SECOND:
        return False
    traffic.uploads.append((now, packet_count, byte_count))
    return True


def _allow_get(client_id: str, now: float) -> bool:
    traffic = _TRAFFIC.setdefault(client_id, Traffic())
    while traffic.gets and now - traffic.gets[0] >= 1.0:
        traffic.gets.popleft()
    if len(traffic.gets) >= MAX_GETS_PER_SECOND:
        return False
    traffic.gets.append(now)
    return True


class RelayHandler(BaseHTTPRequestHandler):
    server_version = "HorizonVoiceRelay/0.1"
    sys_version = ""

    def log_message(self, format_string: str, *args: Any) -> None:
        # Never log request headers (especially bearer credentials) or voice data.
        sys.stderr.write("[voice-relay] " + format_string % args + "\n")

    def _reply(self, status: int, body: bytes = b"", content_type: str = "application/octet-stream") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        if body:
            self.wfile.write(body)

    def _identity(self) -> tuple[str, str, str] | None:
        client_id = self.headers.get("X-HVC-Client", "")
        session_id = self.headers.get("X-HVC-Session", "")
        room_id = self.headers.get("X-HVC-Room", "")
        if not safe_id(client_id) or not SESSION_RE.fullmatch(session_id) or not safe_id(room_id):
            self._reply(400)
            return None
        auth = self.headers.get("Authorization", "")
        if not auth.startswith("Bearer "):
            self._reply(401)
            return None
        supplied_token = auth[7:]
        configured = self.server.clients.get(client_id)  # type: ignore[attr-defined]
        if configured is None or not hmac.compare_digest(configured.token, supplied_token):
            self._reply(401)
            return None
        if room_id not in configured.rooms:
            self._reply(403)
            return None
        return client_id, session_id, room_id

    def do_POST(self) -> None:
        if self.path != "/v1/frames":
            self._reply(404)
            return
        identity = self._identity()
        if identity is None:
            return
        client_id, session_id, room_id = identity
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self._reply(400)
            return
        if not 6 <= length <= MAX_BODY_BYTES:
            self._reply(413 if length > MAX_BODY_BYTES else 400)
            return
        body = self.rfile.read(length)
        if len(body) != length:
            self._reply(400)
            return

        now = time.monotonic()
        sequence_key = (client_id, session_id)
        with _LOCK:
            # Expire sequence state and cap concurrent/recent sessions per
            # enrolled identity. An authenticated client cannot grow this map
            # without bound by repeatedly changing X-HVC-Session.
            stale_sessions = [
                key for key, value in _SEQUENCES.items() if now - value[1] > 30.0
            ]
            for key in stale_sessions:
                _SEQUENCES.pop(key, None)
            active_sessions = sum(1 for key in _SEQUENCES if key[0] == client_id)
            if sequence_key not in _SEQUENCES and active_sessions >= 8:
                self._reply(429)
                return
            current_sequence = _SEQUENCES.get(sequence_key)
            previous = current_sequence[0] if current_sequence else None
            # A new process gets a new session ID; the sequence guard is scoped
            # to that session so restarting cannot inherit stale sequence state.
            try:
                packets, last_sequence = decode_upload(body, previous)
            except ValueError:
                self._reply(400)
                return
            if not _allow_upload(client_id, len(packets), len(body), now):
                self._reply(429)
                return
            room = _ROOMS.get(room_id)
            if room is None:
                if len(_ROOMS) >= 64:
                    self._reply(503)
                    return
                room = _ROOMS.setdefault(room_id, Room())
            _prune_room(room, now)
            if client_id not in room.members and len(room.members) >= MAX_ROOM_CLIENTS:
                self._reply(429)
                return
            room.members[client_id] = now
            _SEQUENCES[sequence_key] = (last_sequence if last_sequence is not None else 0, now)
            current_slot = time.monotonic_ns() // 10_000_000
            for index, packet in enumerate(packets):
                room.cursor += 1
                frame_slot = max(0, current_slot - (len(packets) - index - 1))
                room.frames.append(
                    FrameRecord(room.cursor, now, frame_slot, client_id, packet)
                )
            _prune_room(room, now)
        self._reply(204)

    def do_GET(self) -> None:
        if self.path != "/v1/frames":
            self._reply(404)
            return
        identity = self._identity()
        if identity is None:
            return
        client_id, _session_id, room_id = identity
        try:
            after = int(self.headers.get("X-HVC-Cursor", "0"))
        except ValueError:
            self._reply(400)
            return
        if not 0 <= after <= 0xFFFFFFFFFFFFFFFF:
            self._reply(400)
            return

        now = time.monotonic()
        with _LOCK:
            if not _allow_get(client_id, now):
                self._reply(429)
                return
            room = _ROOMS.get(room_id)
            if room is None:
                body = encode_download(after, [], client_id, time.monotonic_ns() // 10_000_000)
            else:
                _prune_room(room, now)
                if client_id not in room.members and len(room.members) >= MAX_ROOM_CLIENTS:
                    self._reply(429)
                    return
                room.members[client_id] = now
                body = encode_download(
                    after,
                    list(room.frames),
                    client_id,
                    time.monotonic_ns() // 10_000_000,
                )
        self._reply(200, body)

    def do_PUT(self) -> None:
        self._reply(405)

    def do_DELETE(self) -> None:
        self._reply(405)

    def do_OPTIONS(self) -> None:
        self._reply(405)


class RelayHTTPServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 64

    def __init__(self, address: tuple[str, int], clients: dict[str, ClientCredential]):
        self.clients = clients
        self._request_slots = threading.BoundedSemaphore(64)
        super().__init__(address, RelayHandler)

    def get_request(self):
        request, address = super().get_request()
        # Avoid holding worker threads indefinitely while a client dribbles
        # a request body or never reads its response.
        request.settimeout(5.0)
        return request, address

    def process_request(self, request, client_address):
        if not self._request_slots.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self._request_slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self._request_slots.release()


def serve() -> None:
    raw_clients = os.environ.get("HVC_RELAY_CLIENTS", "")
    if not raw_clients:
        raise SystemExit("Set HVC_RELAY_CLIENTS to a JSON client/token/room allow-list.")
    clients = parse_clients(raw_clients)
    cert = os.environ.get("HVC_TLS_CERT", "")
    key = os.environ.get("HVC_TLS_KEY", "")
    if not cert or not key:
        raise SystemExit("TLS is required; set HVC_TLS_CERT and HVC_TLS_KEY.")
    host = os.environ.get("HVC_BIND", "0.0.0.0")
    port = int(os.environ.get("HVC_PORT", "8443"))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certfile=cert, keyfile=key)

    server = RelayHTTPServer((host, port), clients)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print(
        f"Horizon Voice Relay listening on https://{host}:{port}/v1/frames "
        f"({len(clients)} configured clients; TLS required)",
        file=sys.stderr,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    serve()
