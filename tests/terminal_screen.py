"""PTY reads may stop anywhere in a cursor-addressed frame."""
import os
import pty
import subprocess
import sys
import time
import unittest
from unittest.mock import patch
from pty_probe import deadline, drain, read_ready, wait_exit
from terminal import screen


class ScreenTests(unittest.TestCase):
    def test_partial_csi_does_not_crash_or_print_escape_parameters(self):
        for prefix in (b'', b'Projects'):
            for sequence in (b'\x1b[30;1H', b'\x1b[?25l', b'\x1b[2J', b'\x1b[0m'):
                for length in range(1, len(sequence)):
                    with self.subTest(prefix=prefix, sequence=sequence, length=length):
                        self.assertEqual(screen(prefix + sequence[:length]), screen(prefix))

    def test_frame_and_status_survive_every_read_boundary(self):
        frame = b'\x1b[2J\x1b[1;1HAttention\x1b[4;1HProjects\x1b[30;1H1 need you'
        for length in range(len(frame) + 1):
            screen(frame[:length])
        rows = screen(frame).splitlines()
        self.assertEqual(rows[0].rstrip(), 'Attention')
        self.assertEqual(rows[3].rstrip(), 'Projects')
        self.assertEqual(rows[29].rstrip(), '1 need you')


class ProbeIOTests(unittest.TestCase):
    def test_spurious_readiness_cannot_block_a_probe(self):
        reader, writer = os.pipe()
        try:
            with deadline(lambda: 'notify probe: spurious readiness read', 1):
                with patch('pty_probe.select.select', return_value=([reader], [], [])):
                    self.assertEqual(read_ready(reader), b'')
        finally:
            os.close(reader)
            os.close(writer)

    def test_eof_ends_drain_even_when_select_stays_ready(self):
        reader, writer = os.pipe()
        os.close(writer)
        try:
            with deadline(lambda: 'notify probe: EOF drain', 1):
                self.assertEqual(drain(reader), b'')
        finally:
            os.close(reader)

    def test_closed_pty_is_a_finished_read(self):
        master, slave = pty.openpty()
        os.close(slave)
        try:
            with deadline(lambda: 'notify probe: PTY hangup read', 1):
                self.assertEqual(read_ready(master), b'')
        finally:
            os.close(master)

    def test_exit_acknowledgement_drains_backpressure_and_uses_poll(self):
        reader, writer = os.pipe()
        child = subprocess.Popen(
            [sys.executable, '-c', 'import os; data=b"x"*1048576;\nwhile data: data=data[os.write(1,data):]'],
            stdout=writer, stderr=subprocess.DEVNULL)
        os.close(writer)
        received = bytearray()
        def pump():
            received.extend(read_ready(reader))
        try:
            # The child cannot finish until the parent drains more than pipe capacity.
            with patch.object(child, 'wait', side_effect=AssertionError('use poll, not timed wait')):
                self.assertEqual(wait_exit(child, pump, 'notify probe: backpressure exit', 5), 0)
            received.extend(drain(reader))
            self.assertEqual(len(received), 1048576)
        finally:
            if child.poll() is None:
                child.kill()
                wait_exit(child, pump, 'notify probe: killed writer cleanup', 5)
            os.close(reader)

    def test_watchdog_names_the_test_and_acknowledgement(self):
        with self.assertRaisesRegex(TimeoutError, 'notifier-test: frame acknowledgement'):
            with deadline(lambda: 'notifier-test: frame acknowledgement', .01):
                time.sleep(.1)


if __name__ == '__main__':
    unittest.main()
