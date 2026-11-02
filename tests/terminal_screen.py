"""PTY reads may stop anywhere in a cursor-addressed frame."""
import unittest
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


if __name__ == '__main__':
    unittest.main()
