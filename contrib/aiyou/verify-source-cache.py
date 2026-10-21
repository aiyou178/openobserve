# Copyright 2026 OpenObserve Inc.
import argparse
import asyncio
import hashlib
import json
import tempfile
from pathlib import Path
from urllib.parse import urlsplit
from playwright.async_api import async_playwright

parser = argparse.ArgumentParser()
parser.add_argument('--rp-url', default='http://127.0.0.1:5080')
parser.add_argument('--old-revision', required=True)
parser.add_argument('--new-revision', required=True)
parser.add_argument('--old-archive', type=Path, required=True)
parser.add_argument('--new-archive', type=Path, required=True)
parser.add_argument('--browser-executable', required=True)
args = parser.parse_args()
base = args.rp_url.rstrip('/')
parsed = urlsplit(base)
if parsed.hostname not in {'127.0.0.1', 'localhost', '::1'} or parsed.username or parsed.password or parsed.query or parsed.fragment:
    raise SystemExit('Only isolated loopback RP URLs are permitted')
for revision in [args.old_revision, args.new_revision]:
    if len(revision) != 40 or any(char not in '0123456789abcdef' for char in revision):
        raise SystemExit('Full source commit revisions are required')
receipts = {}

def record(name, value):
    receipts[name] = bool(value)
    print(name + ': ' + str(bool(value)), flush=True)
    assert value, name

async def fetch(page, url):
    return await page.evaluate('''async url => {
      const response = await fetch(url);
      const data = new Uint8Array(await response.arrayBuffer());
      const digest = await crypto.subtle.digest('SHA-256', data);
      return {status: response.status, cache: response.headers.get('cache-control'),
        gzip: data[0] === 31 && data[1] === 139,
        hash: Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, '0')).join('')};
    }''', url)

async def main():
    with tempfile.TemporaryDirectory(prefix="oidc-source-cache-") as profile:
        async with async_playwright() as playwright:
            context = await playwright.chromium.launch_persistent_context(profile, executable_path=args.browser_executable, headless=True, args=["--disk-cache-size=1073741824"])
            page = await context.new_page()
            cdp = await context.new_cdp_session(page)
            await cdp.send('Network.enable')
            urls = {}
            cached = set()
            cdp.on('Network.requestWillBeSent', lambda event: urls.update({event['requestId']: event['request']['url']}))
            cdp.on('Network.requestServedFromCache', lambda event: cached.add(urls.get(event['requestId'])))
            cdp.on('Network.responseReceived', lambda event: cached.add(event['response']['url']) if event['response'].get('fromDiskCache') else None)
            await page.goto(base + '/web/login', wait_until='domcontentloaded')
            old_url = base + '/web/assets/source-' + args.old_revision + '.tar.gz'
            old_hash = hashlib.sha256(args.old_archive.read_bytes()).hexdigest()
            new_hash = hashlib.sha256(args.new_archive.read_bytes()).hexdigest()
            record('distinct_revision_archive_bytes', args.old_revision != args.new_revision and old_hash != new_hash)
            first = await fetch(page, old_url)
            record('old_revision_exact_archive', first['status'] == 200 and first['gzip'] and first['hash'] == old_hash)
            record('old_revision_is_immutable', 'immutable' in (first['cache'] or '') and '31536000' in first['cache'])
            second = await fetch(page, old_url)
            record('old_revision_cache_reused', old_url in cached and second['hash'] == old_hash)
            await page.reload(wait_until='domcontentloaded')
            source = page.get_by_role('link', name='Source', exact=True)
            await source.wait_for()
            new_url = await source.get_attribute('href')
            record('new_source_link_contains_exact_revision', urlsplit(new_url).path == parsed.path + '/web/assets/source-' + args.new_revision + '.tar.gz' and new_url != old_url)
            current = await fetch(page, new_url)
            record('new_revision_download_escapes_old_cache', current['status'] == 200 and current['gzip'] and current['hash'] == new_hash and current['hash'] != old_hash)
            again = await fetch(page, old_url)
            record('old_cached_archive_remains_separate', old_url in cached and again['hash'] == old_hash)
            await context.close()

try:
    asyncio.run(main())
except Exception as error:
    print('Source cache verification stopped with exception type: ' + type(error).__name__, flush=True)
    raise SystemExit(1)
print(json.dumps(receipts, sort_keys=True))
