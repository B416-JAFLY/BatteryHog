"""Integration tests for the packaged GPUI companion's actual pipe protocol.

BATTERY_HOG_TEST_SERVICE must identify the executable inside a built .app;
macOS notification services cannot initialize from an unbundled CLI binary.
"""
import json
import os
import selectors
import subprocess
import tempfile
import time
import unittest
from pathlib import Path


@unittest.skipUnless(os.environ.get('BATTERY_HOG_TEST_SERVICE'), 'requires a packaged GPUI app')
class GPUIServiceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='batteryhog-ipc-')
        self.process = subprocess.Popen(
            [os.environ['BATTERY_HOG_TEST_SERVICE'], '-AppleLanguages', '(en)'],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env={**os.environ, 'BATTERY_HOG_DATA_DIR': self.directory.name, 'BATTERY_HOG_APP_BUNDLE': str(Path(os.environ['BATTERY_HOG_TEST_SERVICE']).resolve().parents[5])}, bufsize=0)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.pending = b''
        self.sequence = 0

    def tearDown(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.process.stdout.close()
        self.selector.close()
        self.directory.cleanup()

    def request(self, method, path, body=None):
        self.sequence += 1
        message = dict(id=self.sequence, method=method, path=path, body=body or {})
        # A single short write must be handled immediately, without filling a pipe buffer.
        self.process.stdin.write((json.dumps(message)+'\n').encode())
        self.process.stdin.flush()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            while b'\n' in self.pending:
                line, self.pending = self.pending.split(b'\n', 1)
                response = json.loads(line)
                if response.get('id') == self.sequence:
                    return response
            if self.selector.select(.5):
                chunk = os.read(self.process.stdout.fileno(), 65536)
                self.assertTrue(chunk, 'service exited before responding')
                self.pending += chunk
        self.fail('service did not respond to a short pipe request')

    def test_settings_roundtrip_and_persistence(self):
        response = self.request('POST', '/api/settings', {
            'low_threshold': 35, 'dev': {'slots': 3}, 'power_refresh_seconds': 2
        })
        self.assertEqual(response['status'], 200)
        response = self.request('GET', '/api/settings')
        self.assertEqual(response['body']['low_threshold'], 35)
        self.assertEqual(response['body']['dev']['slots'], 3)
        self.assertEqual(response['body']['power_refresh_seconds'], 2)
        # Unsupported intervals do not modify the chosen value.
        self.assertEqual(self.request('POST', '/api/settings', {'power_refresh_seconds': 3})['status'], 200)
        self.assertEqual(self.request('GET', '/api/settings')['body']['power_refresh_seconds'], 2)
        saved = json.loads((Path(self.directory.name)/'settings.json').read_text())
        self.assertEqual(saved['low_threshold'], 35)
        self.assertEqual(saved['power_refresh_seconds'], 2)

    def test_history_ranges_and_route_validation(self):
        for period, buckets in [('24h', 96), ('10d', 80)]:
            response = self.request('GET', '/api/history?range='+period)
            self.assertEqual(response['body']['range'], period)
            self.assertEqual(len(response['body']['bins']), buckets)
            self.assertGreater(response['body']['end'], response['body']['start'])
        self.assertEqual(self.request('POST', '/api/untrusted')['status'], 400)

    def test_automatic_power_events(self):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            while b'\n' in self.pending:
                line, self.pending = self.pending.split(b'\n', 1)
                event = json.loads(line)
                if event.get('event') == 'power':
                    self.assertIn('power', event['body'])
                    self.assertIn('battery', event['body'])
                    self.assertGreater(event['body']['ts'], 0)
                    return
            if self.selector.select(.5):
                chunk = os.read(self.process.stdout.fileno(), 65536)
                self.assertTrue(chunk, 'service exited before sending power samples')
                self.pending += chunk
        self.fail('no automatic power event within 10 seconds')

    def test_pipe_eof_terminates_companion(self):
        self.request('GET', '/api/settings')
        self.process.stdin.close()
        self.process.wait(timeout=5)
        self.assertEqual(self.process.returncode, 0)


if __name__ == '__main__':
    unittest.main()
