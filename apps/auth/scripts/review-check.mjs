import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, readdir } from 'node:fs/promises';
import { chromium } from 'playwright';

process.chdir(fileURLToPath(new URL('../',import.meta.url)));
const origin='http://127.0.0.1:4176';
const server=spawn(process.execPath,['node_modules/vite/bin/vite.js','preview','--host','127.0.0.1','--port','4176','--strictPort'],{stdio:'pipe'});
let output='';server.stdout.on('data',d=>output+=d);server.stderr.on('data',d=>output+=d);
let browser;
try {
  for(let n=0;n<80;n++) { if(server.exitCode!==null) throw new Error(output); try { if(output.includes('Local:') && (await fetch(origin)).ok)break; } catch {} await new Promise(r=>setTimeout(r,100)); }
  browser=await chromium.launch({headless:true});
  const errors=[]; const calls=[];
  const account={id:'test-account',display_name:'Alex Morgan',verified_email:'alex@example.com',avatar_url:null,linked_methods:['google','passkey']};
  let signedIn=false, fail=false, invoiceState='pending';
  let identities=[{id:'google-test',kind:'google',label:'alex@example.com'},{id:'passkey-test',kind:'passkey',label:'MacBook Pro'}];
  const page=await browser.newPage({viewport:{width:1280,height:900},colorScheme:'light'});
  page.on('pageerror',e=>errors.push(e.message));
  await page.route('https://api.axiom.stream/**',async route=>{
    const req=route.request(), path=new URL(req.url()).pathname, body=req.postDataJSON();
    calls.push({path,method:req.method(),body,csrf:req.headers()['x-csrf-token']});
    let value={ok:true},status=200;
    if(fail && req.method()!=='GET') { status=503;value={code:'service_unavailable'}; }
    else if(path.endsWith('/auth/session')) value=signedIn?{authenticated:true,account,csrf_token:'test-csrf-token'}:{authenticated:false};
    else if(path.endsWith('/password/login') || path.endsWith('/password/login/resend')) {status=202;value={authenticated:false,challenge_id:'b'.repeat(32),expires_at:new Date(Date.now()+600000).toISOString(),resend_after_seconds:60};}
    else if(path.endsWith('/password/login/verify')) {if(body?.code==='123456'){signedIn=true;value={authenticated:true,account,csrf_token:'test-csrf-token'};}else{status=401;value={code:'invalid_login_code'};}}
    else if(path.endsWith('/auth/identities')) value={identities};
    else if(path.includes('/auth/identities/') && req.method()==='DELETE') identities=identities.filter(i=>!path.endsWith(i.id));
    else if(path.endsWith('/billing/status')) value={available_microusd:12400000,posted_microusd:12400000,held_microusd:0,ledger_sequence:3,currency:'microUSD',invoices_enabled:false,invoice_min_usd_cents:100,invoice_max_usd_cents:10000,settlement_asset:null,payment_account:{network:'mainnet',asset:'ZEC',conversion_status:'none',valuation_enabled:true}};
    else if(path.includes('/billing/invoices/')) value={id:'invoice-test',amount_usd_cents:2500,state:invoiceState,expires_at:null,paid_at:null};
    else if(path.endsWith('/approve')) value={status:'approved',callback_uri:'https://evil.invalid/callback'};
    else if(path.includes('/native/authorizations/')) value={id:'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',status:'pending',client_name:'Axiom Desktop',client_platform:'Windows',user_code:'K7MT-P2RV',expires_at:'2099-01-01T00:00:00Z'};
    else if(path.endsWith('/logout')||path.endsWith('/logout-all')||(path.endsWith('/auth/account')&&req.method()==='DELETE')) signedIn=false;
    await route.fulfill({status,contentType:'application/json',headers:{'access-control-allow-origin':origin,'access-control-allow-credentials':'true','access-control-allow-methods':'GET,POST,DELETE,OPTIONS','access-control-allow-headers':'content-type,x-csrf-token'},body:JSON.stringify(value)});
  });
  const go=async path=>{await page.goto(origin+path);await page.locator('h1').waitFor();};
  const native='/native/authorize?authorization_id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&state=valid-state-with-at-least-thirtytwo-characters';
  await go(native);
  const signup=await page.getByRole('link',{name:'Create an account'}).getAttribute('href');
  assert.equal(new URL(signup,origin).searchParams.get('return_to'),native,'signup must preserve desktop authorization');
  await page.getByRole('link',{name:'Create an account'}).click();
  assert.equal(new URL(await page.getByRole('link',{name:'Sign in',exact:true}).getAttribute('href'),origin).searchParams.get('return_to'),native);
  await page.getByRole('link',{name:'Sign in',exact:true}).click();
  await page.getByLabel('Email address',{exact:true}).fill('alex@example.com');
  await page.getByLabel('Password',{exact:true}).fill('a-long-test-password');
  await page.getByRole('button',{name:'Sign in',exact:true}).click();
  await page.getByLabel('Email sign-in code').waitFor();
  assert.equal(signedIn,false,'password alone must not create a session');
  assert(await page.getByRole('button',{name:/Resend code in/}).isDisabled());
  await page.getByLabel('Email sign-in code').fill('000000');
  await page.getByRole('button',{name:'Verify and sign in'}).click();
  await page.getByRole('alert').filter({hasText:'That code is incorrect'}).waitFor();
  assert.equal(signedIn,false);assert.equal(await page.getByLabel('Email sign-in code').inputValue(),'');
  await page.getByLabel('Email sign-in code').fill('123456');
  await page.getByRole('button',{name:'Verify and sign in'}).click();
  await page.getByRole('heading',{name:'Meet your desktop.'}).waitFor();
  await page.getByRole('button',{name:'Connect desktop'}).click();
  await page.getByRole('heading',{name:'You’re all set.'}).waitFor();
  assert.equal(new URL(page.url()).origin,origin,'unsafe callback must not navigate');
  const approval=calls.find(c=>c.path.endsWith('/approve'));
  assert.equal(approval.csrf,'test-csrf-token');assert.equal(approval.body.state,new URL(native,origin).searchParams.get('state'));
  console.log('PASS required email code, rejected wrong code, native return paths, CSRF and unsafe callback handling');

  await go('/account');await page.getByRole('button',{name:'Remove',exact:true}).first().waitFor();
  await page.getByRole('button',{name:'Remove',exact:true}).first().click();
  const dialog=page.getByRole('dialog');await dialog.waitFor();
  assert.equal(await page.evaluate(()=>document.activeElement.textContent),'Cancel');
  await page.keyboard.press('Escape');await dialog.waitFor({state:'hidden'});
  assert.equal(calls.filter(c=>c.method==='DELETE').length,0);
  await page.getByRole('button',{name:'Remove',exact:true}).first().click();
  await dialog.getByRole('button',{name:'Remove method'}).click();await dialog.waitFor({state:'hidden'});
  await page.waitForFunction(()=>document.querySelectorAll('.identity-row').length===1);
  assert(await page.getByRole('button',{name:'Remove',exact:true}).isDisabled());
  await page.getByRole('button',{name:'Delete account',exact:true}).click();await dialog.waitFor();
  await dialog.getByRole('button',{name:'Cancel'}).click();
  await page.getByRole('button',{name:'Sign out everywhere',exact:true}).click();await dialog.waitFor();
  await dialog.getByRole('button',{name:'Sign out everywhere',exact:true}).click();
  await page.getByRole('heading',{name:'Welcome back.'}).waitFor();
  assert.equal(calls.find(c=>c.path.endsWith('/logout-all')).body.confirmation,'LOG_OUT_ALL');
  console.log('PASS account confirmations, focus, cancellation, last-method protection and global sign-out');

  await go('/recover');await page.getByLabel('Email address').fill('unknown@example.com');
  await page.getByRole('button',{name:'Send recovery link'}).click();await page.getByRole('heading',{name:'Check your inbox.'}).waitFor();
  await go('/verify-email#token=test-verification-token-not-real');
  await page.getByLabel('New password',{exact:true}).fill('a-long-test-password');
  await page.getByLabel('Confirm password',{exact:true}).fill('different-long-password');
  await page.getByRole('button',{name:'Finish verification'}).click();
  assert(await page.getByRole('alert').filter({hasText:'Passwords do not match.'}).isVisible());
  assert.equal(calls.filter(c=>c.path.endsWith('/password/verify')).length,0);
  await page.getByLabel('Confirm password',{exact:true}).fill('a-long-test-password');
  await page.getByRole('button',{name:'Finish verification'}).click();await page.getByRole('heading',{name:'You’re ready.'}).waitFor();
  assert.equal(new URL(page.url()).hash,'');
  assert.equal(calls.find(c=>c.path.endsWith('/password/verify')).body.token,'test-verification-token-not-real');
  await go('/reset-password#token=test-password-reset-token-not-real');
  await page.getByLabel('New password',{exact:true}).fill('a-long-new-test-password');
  await page.getByLabel('Confirm new password',{exact:true}).fill('a-long-new-test-password');
  await page.getByRole('button',{name:'Change password'}).click();await page.getByRole('link',{name:'Continue to sign in'}).waitFor();
  assert.equal(new URL(page.url()).hash,'');
  fail=true;await go('/signin');await page.getByLabel('Email address',{exact:true}).fill('alex@example.com');await page.getByLabel('Password',{exact:true}).fill('a-long-test-password');await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.getByRole('alert').waitFor();fail=false;
  console.log('PASS recovery, password confirmation, one-time token handling and API error presentation');

  signedIn=true;await go('/payments/return?invoice_id=invoice-test&state=paid');
  await page.getByRole('heading',{name:'Waiting for confirmation'}).waitFor();invoiceState='paid';await page.getByRole('button',{name:'Refresh status'}).click();await page.getByRole('heading',{name:'Payment confirmed'}).waitFor();
  await page.getByRole('button',{name:'Use dark theme'}).click();assert.equal(await page.locator('html').getAttribute('data-theme'),'dark');
  assert.deepEqual(await page.evaluate(()=>[localStorage.length,sessionStorage.length]),[0,0]);
  await go('/not-a-page');await page.getByRole('heading',{name:'A little off course.'}).waitFor();
  assert.deepEqual(errors,[]);
  const js=(await readdir('dist/assets')).filter(f=>f.endsWith('.js'));
  for(const name of js) { const source=await readFile(`dist/assets/${name}`,'utf8'); for(const marker of ['alex@example.com','Sample data','preview-token-not-a-credential','Design preview']) assert(!source.includes(marker),`production bundle contains ${marker}`); }
  console.log('PASS server-authoritative payment status, theme, empty browser storage and preview exclusion');
} finally { await browser?.close();server.kill('SIGTERM'); }
