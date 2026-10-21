# Copyright 2026 OpenObserve Inc.
import asyncio,json,base64,argparse,tarfile,io,hashlib,os,signal,subprocess
import re
from urllib.parse import urlsplit
from pathlib import Path
from playwright.async_api import async_playwright
parser=argparse.ArgumentParser()
parser.add_argument('--fixture-dir',type=Path,required=True)
parser.add_argument('--provider-login',type=Path,required=True)
parser.add_argument('--browser-executable',required=True)
parser.add_argument('--rp-url',default='http://127.0.0.1:5080')
parser.add_argument('--recovery-case',choices=['membership','binding','disabled'])
parser.add_argument('--restart-helper',type=Path)
parser.add_argument('--source-archive',type=Path)
args=parser.parse_args()
BASE=args.rp_url.rstrip('/')
parsed=urlsplit(BASE)
if parsed.hostname not in {'127.0.0.1','localhost','::1'} or parsed.username or parsed.password or parsed.query or parsed.fragment:
 raise SystemExit('Only isolated loopback RP URLs are permitted')
root=args.fixture_dir
provider=json.loads(args.provider_login.read_text())
local=json.loads((root/'local-credentials.json').read_text())
binding=json.loads((root/'binding-manifest.json').read_text())
if binding.get('disposable') is not True:
 raise SystemExit('A disposable fixture manifest is required')
receipts={}
def record(name,value):
 receipts[name]=value
 print(name+': '+str(value),flush=True)
 assert value,name
async def main():
 async with async_playwright() as p:
  browser=await p.chromium.launch(executable_path=args.browser_executable,headless=True,args=['--disable-background-networking'])
  context=await browser.new_context()
  page=await context.new_page()
  await page.goto(BASE+'/web/login',wait_until='domcontentloaded')
  button=page.locator('[data-test="oidc-sso-login"]')
  await button.wait_for(timeout=30000)
  record('rp_login_action',await button.is_visible())
  record('network_visible_login_source',await page.get_by_role('link',name='Source',exact=True).is_visible())
  source_link=await page.get_by_role('link',name='Source',exact=True).get_attribute('href')
  await button.click()
  await page.locator('#username').wait_for(timeout=30000)
  await page.locator('#username').fill(provider['username'])
  await page.locator('#password').fill(provider['password'])
  await page.get_by_role('button',name=re.compile(r'登\s*录')).click()
  await page.wait_for_timeout(2000)
  if await page.get_by_role("button",name=re.compile(r"允\s*许")).is_visible():
   await page.get_by_role("button",name=re.compile(r"允\s*许")).click()
   record("explicit_test_client_consent",True)
  await page.wait_for_function("base => { const prefix = new URL(base).pathname; return window.location.origin === new URL(base).origin && window.location.pathname.startsWith((prefix === '/' ? '' : prefix) + '/web'); }",arg=BASE,timeout=30000)
  bootstrap=None
  for _ in range(40):
   response=await context.request.get(BASE+'/auth/oidc/bootstrap',headers={'x-o2-bootstrap':'1'})
   if response.ok:
    bootstrap=await response.json()
    if bootstrap.get('authenticated'):break
   await page.wait_for_timeout(250)
  record('actual_provider_login_and_rp_session',bool(bootstrap and bootstrap.get('authenticated')))
  record('issuer_sub_maps_to_explicit_local_user',bootstrap['user']['email']==local['user_email'])
  record('clean_browser_callback_url',not any(item in page.url for item in ['code=','state=','nonce=']))
  await page.locator('[data-test="menu-link-help-item"]').wait_for(timeout=30000)
  await page.locator('[data-test="menu-link-help-item"]').click()
  record('authenticated_source_menu',await page.locator('[data-test="menu-link-source-item"]').is_visible())
  await page.keyboard.press('Escape')
  cookies=await context.cookies(BASE)
  session=next(cookie for cookie in cookies if cookie['name']=='o2_oidc_session')
  record('opaque_httponly_lax_session',len(session['value'])==43 and session['httpOnly'] and session['sameSite']=='Lax')
  if args.source_archive:
   response=await context.request.get(source_link)
   archive=await response.body()
   record('source_download_is_exact_archive',response.status==200 and archive[:2]==b'\x1f\x8b' and hashlib.sha256(archive).digest()==hashlib.sha256(args.source_archive.read_bytes()).digest())
   with tarfile.open(fileobj=io.BytesIO(archive),mode='r:gz') as source:
    names=source.getnames()
    record('source_download_contains_code_license_and_build',all(any(name.endswith(expected) for name in names) for expected in ['Cargo.toml','LICENSE','contrib/aiyou/Dockerfile.oidc']))
  if args.recovery_case:
   auth='Basic '+base64.b64encode((local['root_email']+':'+local['root_password']).encode()).decode()
   machine=await p.request.new_context(extra_http_headers={'Authorization':auth})
   config_path=root/'oidc.json'
   original_config=config_path.read_bytes()
   removed=False
   try:
    if args.recovery_case=='membership':
     response=await machine.delete(BASE+'/api/'+binding['allowed_org']+'/users/'+local['user_email'])
     record('recovery_membership_removed',response.status==200)
     removed=True
    elif args.recovery_case=='binding':
     settings=json.loads(original_config);settings['bindings']=[]
     config_path.write_text(json.dumps(settings))
    else:
     if not args.restart_helper:raise RuntimeError('Restart helper required')
     os.kill(int((root/'server.pid').read_text()),signal.SIGTERM)
     await asyncio.sleep(3)
     subprocess.run(['python3',str(args.restart_helper)],env={**os.environ,'OIDC_TEST_ENABLED':'false'},check=True,stdout=subprocess.DEVNULL)
     for _ in range(120):
      try:
       health=await context.request.get(BASE+'/healthz',timeout=1000)
       if health.ok:break
      except Exception:pass
      await asyncio.sleep(1)
    response=await context.request.get(BASE+'/auth/oidc/bootstrap',headers={'x-o2-bootstrap':'1'})
    recovery=await response.json()
    record('unauthorized_session_has_logout_proof',not recovery.get('authenticated') and recovery.get('session_invalid') and recovery.get('csrf')==bootstrap['csrf'])
    if args.recovery_case=='disabled':
     response=await context.request.post(BASE+'/auth/oidc/login',data={})
     record('disabled_new_login',not recovery['enabled'] and response.status in {401,404,405} and 'location' not in response.headers)
    response=await context.request.post(BASE+'/auth/oidc/logout',headers={'Origin':BASE})
    record('recovery_missing_csrf_denied',response.status==403)
    response=await context.request.post(BASE+'/auth/oidc/logout',headers={'Origin':'https://attacker.example','x-o2-csrf':recovery['csrf']})
    record('recovery_foreign_origin_denied',response.status==403)
    response=await context.request.post(BASE+'/auth/oidc/logout',headers={'Origin':BASE,'x-o2-csrf':recovery['csrf']})
    record('unauthorized_session_logout_succeeds',response.status==204)
    record('recovery_clears_all_auth_cookies',not any(cookie['name'] in {'o2_oidc_session','o2_oidc_browser','auth_tokens','auth_ext'} for cookie in await context.cookies(BASE)))
    replay=await p.request.new_context()
    response=await replay.get(BASE+'/api/'+binding['allowed_org']+'/streams',headers={'Cookie':'o2_oidc_session='+session['value']})
    record('recovery_revoked_cookie_denied',response.status==401)
    await replay.dispose()
    response=await context.request.post(BASE+'/auth/login',data={'name':local['root_email'],'password':local['root_password']})
    record('recovery_native_login',response.status==200)
    response=await context.request.get(BASE+'/api/organizations')
    record('recovery_native_cookie_api',response.status==200)
   finally:
    config_path.write_bytes(original_config)
    if removed:
     response=await machine.post(BASE+'/api/'+binding['allowed_org']+'/users/'+local['user_email'],data={'role':'admin'})
     record('recovery_membership_restored',response.status==200)
    if args.recovery_case=='disabled':
     os.kill(int((root/'server.pid').read_text()),signal.SIGTERM)
     await asyncio.sleep(3)
     subprocess.run(['python3',str(args.restart_helper)],check=True,stdout=subprocess.DEVNULL)
    await machine.dispose()
   for _ in range(120):
    try:
     health=await context.request.get(BASE+'/healthz',timeout=1000)
     if health.ok:break
    except Exception:pass
    await asyncio.sleep(1)
   replay=await p.request.new_context()
   response=await replay.get(BASE+'/api/'+binding['allowed_org']+'/streams',headers={'Cookie':'o2_oidc_session='+session['value']})
   record('recovery_revoked_cookie_denied_after_authorization_restored',response.status==401)
   await replay.dispose();await browser.close()
   (root/('recovery-'+args.recovery_case+'-receipts.json')).write_text(json.dumps(receipts,indent=2))
   return
  csrf=bootstrap['csrf']
  allowed=binding['allowed_org'];denied=binding['denied_org']
  response=await context.request.get(BASE+'/api/organizations');data=await response.json()
  record('organization_list_intersection',response.status==200 and [o['identifier'] for o in data['data']]==[allowed])
  response=await context.request.get(BASE+'/api/'+allowed+'/streams')
  record('allowed_organization_api',response.status==200)
  response=await context.request.get(BASE+'/api/'+denied+'/streams')
  record('unlisted_existing_membership_denied',response.status==401)
  response=await context.request.post(BASE+'/api/'+allowed+'/_search',headers={'Origin':BASE},data={'query':{'sql':'select 1'}})
  record('missing_csrf_write_denied',response.status==401)
  response=await context.request.post(BASE+'/api/'+allowed+'/_search',headers={'Origin':'https://attacker.example','x-o2-csrf':csrf},data={'query':{'sql':'select 1'}})
  record('foreign_origin_write_denied',response.status==401)
  response=await context.request.post(BASE+'/api/'+allowed+'/v1/logs',headers={'Origin':BASE,'x-o2-csrf':csrf},data={})
  record('human_cookie_cannot_ingest',response.status==401 and 'location' not in response.headers)
  response=await context.request.post(BASE+'/auth/login',data={'name':local['root_email'],'password':local['root_password']})
  record('legacy_native_login_remains_usable',response.status==200)
  response=await context.request.get(BASE+'/api/organizations');data=await response.json()
  record('stale_root_legacy_cookie_cannot_switch_sso_identity',response.status==200 and [o['identifier'] for o in data['data']]==[allowed])
  auth='Basic '+base64.b64encode((local['root_email']+':'+local['root_password']).encode()).decode()
  response=await context.request.get(BASE+'/api/organizations',headers={'Authorization':auth});data=await response.json()
  record('explicit_machine_authorization_has_separate_precedence',response.status==200 and len(data['data'])>1)
  machine=await p.request.new_context(extra_http_headers={'Authorization':auth})
  response=await machine.post(BASE+'/api/'+allowed+'/oidc_smoke/_json',data=[{'disposable':'oidc-interop'}])
  record('machine_native_ingestion_remains_usable',response.status==200)
  response=await machine.delete(BASE+'/api/'+allowed+'/users/'+local['user_email'])
  record('disposable_membership_removal',response.status==200)
  response=await context.request.get(BASE+'/api/organizations')
  record('current_membership_rechecked_on_next_request',response.status==401)
  response=await machine.post(BASE+'/api/'+allowed+'/users/'+local['user_email'],data={'role':'admin'})
  record('disposable_membership_restored',response.status==200)
  await page.locator('[data-test="header-my-account-profile-icon"]').click()
  await page.locator('[data-test="menu-link-logout-item"]').click()
  await page.locator('[data-test="oidc-sso-login"]').wait_for(timeout=30000)
  response=await context.request.get(BASE+'/auth/oidc/bootstrap',headers={'x-o2-bootstrap':'1'})
  after_logout=await response.json()
  record('frontend_logout_revokes_session',response.status==200 and not after_logout.get('authenticated'))
  replay=await p.request.new_context()
  response=await replay.get(BASE+'/api/'+allowed+'/streams',headers={'Cookie':'o2_oidc_session='+session['value']})
  await replay.dispose()
  record('revoked_cookie_replay_denied',response.status==401)
  await machine.dispose();await browser.close()
 (root/'interop-receipts.json').write_text(json.dumps(receipts,indent=2))
try:asyncio.run(main())
except Exception as error:
 (root/'interop-receipts.json').write_text(json.dumps(receipts,indent=2))
 print('Interop stopped with exception type: '+type(error).__name__,flush=True)
 raise SystemExit(1)
