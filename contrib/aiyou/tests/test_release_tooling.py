# Copyright 2026 OpenObserve Inc.
import functools
import gzip
import http.server
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[3]
TOOLS = ROOT / 'contrib/aiyou'
VERSION = (TOOLS / 'VERSION').read_text().strip()


class ImmutableAssets(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        if self.path.startswith('/web/assets/'):
            self.send_header('Cache-Control', 'public, max-age=31536000, immutable')
        super().end_headers()


class ReleaseToolingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='oidc-release-contract-')
        self.addCleanup(self.temporary.cleanup)
        self.work = Path(self.temporary.name)
        self.repository = self.work / 'project'
        self.repository.mkdir()
        self.git('init', '--quiet')
        for key, value in [('user.name', 'Release contract'),
                           ('user.email', 'release@example.invalid'),
                           ('commit.gpgsign', 'false'),
                           ('core.hooksPath', '/dev/null')]:
            self.git('config', key, value)
        (self.repository / 'contrib/aiyou').mkdir(parents=True)
        shutil.copyfile(ROOT / 'rust-toolchain.toml', self.repository / 'rust-toolchain.toml')

    def git(self, *arguments):
        return subprocess.check_output(['git', *arguments], cwd=self.repository, text=True).strip()

    def revision(self, version):
        (self.repository / 'contrib/aiyou/VERSION').write_text(version + '\n')
        self.git('add', '.')
        self.git('commit', '--quiet', '-m', 'Fixture ' + version)
        return self.git('rev-parse', 'HEAD')

    def test_helpers_validate_versions_before_docker_preflight(self):
        commands = self.work / 'bin'
        commands.mkdir()
        docker = commands / 'docker'
        docker.write_text('#!/bin/sh\nprintf reached > "$DOCKER_PROBE"\nexit 73\n')
        docker.chmod(0o755)
        probe = self.work / 'docker-probe'
        environment = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'],
                           DOCKER_PROBE=str(probe))
        for version, supported in [('v1.1.2.5', True), (VERSION, True),
                                   ('v2.0.0.1', True), ('v1.2.0', False),
                                   ('v1.2.0.0', False), ('v1.2.0.1-beta', False)]:
            self.revision(version)
            for helper in ['build-binaries.sh', 'build-oidc-image.sh']:
                with self.subTest(version=version, helper=helper):
                    probe.unlink(missing_ok=True)
                    result = subprocess.run(['bash', str(TOOLS / helper), 'amd64'],
                                            cwd=self.repository, env=environment,
                                            capture_output=True, text=True, timeout=15)
                    if supported:
                        self.assertEqual(result.returncode, 73, result.stderr)
                        self.assertTrue(probe.is_file(), result.stderr)
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertFalse(probe.exists(), result.stderr)

    def test_source_cache_keeps_historical_and_current_archives_independent(self):
        browser = (os.environ.get('OIDC_TEST_BROWSER_EXECUTABLE') or shutil.which('chromium')
                   or shutil.which('google-chrome')
                   or '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome')
        self.assertTrue(Path(browser).is_file(), 'Set OIDC_TEST_BROWSER_EXECUTABLE to Chromium')
        old_revision = self.revision('v1.1.2.5')
        new_revision = self.revision(VERSION)
        assets = self.work / 'web/assets'
        assets.mkdir(parents=True)
        archives = []
        for revision, version in [(old_revision, 'v1.1.2.5'), (new_revision, VERSION)]:
            archive = assets / f'openobserve-source-{revision}-{version}.tar.gz'
            source = subprocess.check_output(['git', 'archive', '--format=tar', revision],
                                             cwd=self.repository)
            archive.write_bytes(gzip.compress(source, mtime=0))
            archives.append(archive)
        login = self.work / 'web/login'
        login.mkdir()
        (login / 'index.html').write_text(
            f'<html><body><a href="/web/assets/{archives[1].name}">Source</a></body></html>')
        handler = functools.partial(ImmutableAssets, directory=str(self.work))
        with http.server.ThreadingHTTPServer(('127.0.0.1', 0), handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                result = subprocess.run(
                    [sys.executable, str(TOOLS / 'verify-source-cache.py'), '--rp-url',
                     f'http://127.0.0.1:{server.server_port}', '--old-revision', old_revision,
                     '--new-revision', new_revision, '--old-archive', str(archives[0]),
                     '--new-archive', str(archives[1]), '--browser-executable', browser],
                    capture_output=True, text=True, timeout=90)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            finally:
                server.shutdown()
                thread.join()

    def test_generated_build_info_header_identifies_release_upstream(self):
        dockerfile = (TOOLS / 'Dockerfile.binaries').read_text()
        block = re.search(r'(?ms)^\s*\{ \\\n(.*?)^\s*\} > /build-output/BUILD-INFO\.txt',
                          dockerfile)
        self.assertIsNotNone(block, 'BUILD-INFO generation block is missing')
        commands = '\n'.join(line.strip().removesuffix(' \\')
                             for line in block[1].splitlines()
                             if line.strip().startswith('printf '))
        environment = dict(os.environ, VERSION=VERSION, SOURCE_REVISION='a' * 40,
                           target='x86_64-unknown-linux-gnu',
                           RUST_TOOLCHAIN_PIN='nightly-2026-10-24',
                           RUST_TOOLCHAIN='nightly-2026-10-24')
        generated = subprocess.check_output(['sh', '-c', commands], env=environment, text=True)
        header = dict(line.split('=', 1) for line in generated.splitlines())
        upstream_tag = '.'.join(VERSION.split('.')[:3])
        expected = subprocess.check_output(['git', 'rev-parse', upstream_tag + '^{}'],
                                           cwd=ROOT, text=True).strip()
        self.assertEqual(header['upstream_base'], expected)


if __name__ == '__main__':
    unittest.main()
