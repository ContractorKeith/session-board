"""Bound test acknowledgements even when a PTY reports spurious readiness."""
from contextlib import contextmanager
import os
import select
import signal
import time


def read_ready(fd, timeout=.005):
    os.set_blocking(fd, False)
    if not select.select([fd], [], [], timeout)[0]:
        return b''
    try:
        return os.read(fd, 65536)
    except BlockingIOError:
        return b''


def drain(fd):
    chunks = []
    until = time.monotonic() + 1
    while time.monotonic() < until:
        chunk = read_ready(fd)
        if not chunk:
            return b''.join(chunks)
        chunks.append(chunk)
    raise TimeoutError('PTY drain did not become quiet within one second')


@contextmanager
def deadline(label, seconds=60):
    previous = signal.getsignal(signal.SIGALRM)
    previous_term = signal.getsignal(signal.SIGTERM)

    def expired(_signal, _frame):
        raise TimeoutError(label())

    signal.signal(signal.SIGALRM, expired)
    signal.signal(signal.SIGTERM, expired)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)
        signal.signal(signal.SIGTERM, previous_term)


def reap(pid, label):
    for sig, seconds in ((signal.SIGTERM, 3), (signal.SIGKILL, 5)):
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            pass
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            try:
                if os.waitpid(pid, os.WNOHANG)[0]:
                    return
            except ChildProcessError:
                return
            time.sleep(.01)
    raise TimeoutError(f'{label}: child {pid} did not exit after SIGKILL')
