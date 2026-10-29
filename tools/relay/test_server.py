#!/usr/bin/env python3
"""Unit tests for the authenticated Horizon voice relay's binary envelope."""
import unittest

import server


def packet(sequence: int = 0, sample_rate: int = 8_000) -> bytes:
    count = (sample_rate + 99) // 100
    payload = sample_rate.to_bytes(4, "little") + count.to_bytes(2, "little")
    payload += bytes(count * 2)
    return (
        b"HVC1"
        + bytes((1, 2))
        + b"\0\0"
        + sequence.to_bytes(4, "little")
        + len(payload).to_bytes(2, "little")
        + payload
    )


class RelayProtocolTests(unittest.TestCase):
    def test_audio_packet_and_batch_round_trip(self) -> None:
        first = packet(9)
        second = packet(10)
        body = b"HVB1" + (2).to_bytes(2, "little")
        body += len(first).to_bytes(2, "little") + first
        body += len(second).to_bytes(2, "little") + second
        frames, last = server.decode_upload(body)
        self.assertEqual(frames, [first, second])
        self.assertEqual(last, 10)

    def test_packet_validation_rejects_malformed_audio(self) -> None:
        good = packet(1)
        self.assertEqual(server.validate_audio_packet(good), (1, len(good)))
        with self.assertRaises(ValueError):
            server.validate_audio_packet(good + b"trailing")
        with self.assertRaises(ValueError):
            server.validate_audio_packet(b"X" + good[1:])
        with self.assertRaises(ValueError):
            server.validate_audio_packet(packet(1, sample_rate=96_000))

    def test_replay_guard_handles_wrap_and_duplicates(self) -> None:
        self.assertTrue(server.is_newer_sequence(0, 0xFFFFFFFF))
        self.assertFalse(server.is_newer_sequence(7, 7))
        self.assertFalse(server.is_newer_sequence(6, 7))
        self.assertFalse(server.is_newer_sequence(0x80000000, 0))

    def test_whole_batch_is_rejected_on_replay_or_trailing_data(self) -> None:
        frame = packet(3)
        body = b"HVB1" + (2).to_bytes(2, "little")
        body += len(frame).to_bytes(2, "little") + frame
        body += len(frame).to_bytes(2, "little") + frame
        with self.assertRaises(ValueError):
            server.decode_upload(body)
        with self.assertRaises(ValueError):
            server.decode_upload(body + b"\0")

    def test_credentials_require_valid_ids_rooms_and_long_tokens(self) -> None:
        token = "a" * 48
        clients = server.parse_clients(
            '{"alice":{"token":"' + token + '","rooms":["survival"]}}'
        )
        self.assertEqual(clients["alice"].rooms, frozenset({"survival"}))
        with self.assertRaises(ValueError):
            server.parse_clients('{"../alice":{"token":"' + token + '","rooms":["survival"]}}')
        with self.assertRaises(ValueError):
            server.parse_clients('{"alice":{"token":"short","rooms":["survival"]}}')
        with self.assertRaises(ValueError):
            server.parse_clients('{"alice":{"token":"' + token + '","rooms":["../world"]}}')

    def test_download_excludes_sender_but_advances_cursor(self) -> None:
        local = packet(0)
        remote = packet(0)
        room_frames = [
            server.FrameRecord(1, 0.0, 10, "alice", local),
            server.FrameRecord(2, 0.0, 10, "bob", remote),
        ]
        body = server.encode_download(0, room_frames, "alice", 18)
        self.assertEqual(body[:4], b"HVR1")
        self.assertEqual(int.from_bytes(body[4:12], "little"), 2)
        self.assertEqual(int.from_bytes(body[12:20], "little"), 18)
        self.assertEqual(int.from_bytes(body[20:22], "little"), 1)
        self.assertEqual(int.from_bytes(body[22:30], "little"), 10)
        self.assertEqual(body[30], 3)
        self.assertEqual(body[31:34], b"bob")


if __name__ == "__main__":
    unittest.main()
