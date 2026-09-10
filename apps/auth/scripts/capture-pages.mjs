import { chromium } from 'playwright';
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { scenarios } from '../review/scenarios.ts';
const origin=process.env.REVIEW_ORIGIN??'http://127.0.0.1:4174';
const directory=fileURLToPath(new URL('../../../dist/auth-review/',import.meta.url));
await mkdir(directory,{recursive:true});
const browser=await chromium.launch({headless:true});
const records=[],errors=[];
try {
 for(const theme of ['light','dark']) {
  for(const [size,viewport] of [['desktop',{width:1440,height:1000}],['mobile',{width:390,height:844}]]) {
   const page=await browser.newPage({viewport,colorScheme:theme,deviceScaleFactor:1});
   page.on('pageerror',e=>errors.push(e.message));
   for(const screen of scenarios) {
    await page.goto(origin+screen.url);
    await page.waitForTimeout(550);
    await page.evaluate(()=>document.fonts.ready);
    await page.addStyleTag({content:'.review-bar{display:none!important} body{padding-bottom:0!important}'});
    const overflow=await page.evaluate(()=>document.documentElement.scrollWidth>window.innerWidth);
    if(overflow)errors.push(`${theme} ${size} ${screen.name}: horizontal overflow`);
    const name=`${screen.name.toLowerCase().replaceAll(/[^a-z0-9]+/g,'-')}-${theme}-${size}.png`;
    await page.screenshot({path:`${directory}/${name}`,fullPage:true});
    records.push({...screen,theme,size,image:name});
   }
   await page.close();console.log(`Captured ${theme} ${size}: ${scenarios.length} pages`);
  }
 }
 // Interaction states use the real components after sample actions.
 const page=await browser.newPage({viewport:{width:1440,height:1000},colorScheme:'light'});
 const capture=async(name)=>{await page.evaluate(()=>document.fonts.ready);await page.addStyleTag({content:'.review-bar{display:none!important}body{padding-bottom:0!important}'});const image=`${name}-light-desktop.png`;await page.screenshot({path:`${directory}/${image}`,fullPage:true});records.push({name:name.replaceAll('-',' '),url:page.url().replace(origin,''),theme:'light',size:'desktop',image});};
 await page.goto(origin+'/account');await page.getByRole('button',{name:'Delete account',exact:true}).click();await page.getByRole('dialog').waitFor();await capture('delete-confirmation');
 await page.getByRole('button',{name:'Cancel',exact:true}).click();await page.getByRole('button',{name:'Sign out everywhere',exact:true}).click();await page.getByRole('dialog').waitFor();await capture('sign-out-confirmation');
 await page.goto(origin+'/recover');await page.getByLabel('Email address').fill('alex@example.com');await page.getByRole('button',{name:'Send recovery link'}).click();await page.getByRole('heading',{name:'Check your inbox.'}).waitFor();await capture('recovery-email-sent');
 await page.goto(origin+'/verify-email#token=preview-token-not-a-credential');await page.getByLabel('New password',{exact:true}).fill('example-password-long');await page.getByLabel('Confirm password',{exact:true}).fill('example-password-long');await page.getByRole('button',{name:'Finish verification'}).click();await page.getByRole('heading',{name:'You’re ready.'}).waitFor();await capture('email-verified');
 await page.goto(origin+'/signup');await page.getByLabel('Email address').fill('alex@example.com');await page.getByRole('button',{name:'Create account',exact:true}).click();await page.getByRole('status').filter({hasText:'Check your email'}).waitFor();await capture('signup-email-sent');
 await page.goto(origin+'/signin');await page.getByLabel('Email address').fill('alex@example.com');await page.getByLabel('Password',{exact:true}).fill('example-password-long');await page.getByRole('button',{name:'Sign in',exact:true}).click();await page.getByLabel('Email sign-in code').waitFor();await capture('email-sign-in-code');
 await page.close();
} finally {await browser.close();}
await writeFile(`${directory}/manifest.json`,JSON.stringify({created:new Date().toISOString(),records,errors},null,2));
const cards=records.map(r=>`<article data-theme="${r.theme}" data-size="${r.size}"><a href="${r.image}"><img src="${r.image}" loading="lazy" alt="${r.name}, ${r.theme}, ${r.size}"></a><div><strong>${r.name}</strong><small>${r.theme} · ${r.size}</small><a href="${origin}${r.url}">Open interactive page ↗</a></div></article>`).join('');
await writeFile(`${directory}/index.html`,`<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Axiom account · Design review</title><style>*{box-sizing:border-box}body{margin:0;padding:48px;background:#f2f4fa;color:#1f1f1f;font:14px system-ui}h1{font:54px Georgia;margin:14px 0}header{margin-bottom:36px}header p{color:#676974;line-height:1.6}nav{display:flex;gap:10px;margin:20px 0}button{padding:10px 16px;border:1px solid #ddd;border-radius:8px;background:white;cursor:pointer}main{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:22px}article{border:1px solid #dddfe5;border-radius:14px;overflow:hidden;background:white}article img{display:block;width:100%;height:290px;object-fit:contain;object-position:top;background:#f2f4fa;border-bottom:1px solid #dddfe5}article>div{padding:17px}article strong{display:block;text-transform:capitalize}small{display:block;color:#787a84;margin:6px 0 12px}a{color:inherit;text-underline-offset:4px;font-size:12px}article[hidden]{display:none}@media(max-width:900px){main{grid-template-columns:repeat(2,minmax(0,1fr))}}@media(max-width:550px){body{padding:22px}main{grid-template-columns:1fr}h1{font-size:38px}}</style><header><small>AXIOM · ACCOUNT REDESIGN</small><h1>A space of your own.</h1><p>Every hosted account page, with Axiom’s original assets, fonts and palette.<br>These previews use sample data. Production authentication has not been replaced.</p><nav><button data-filter="light-desktop">Light · desktop</button><button data-filter="dark-desktop">Dark · desktop</button><button data-filter="light-mobile">Mobile</button><button data-filter="all">All pages</button></nav></header><main>${cards}</main><script>function filter(value){document.querySelectorAll('article').forEach(card=>card.hidden=value!=='all'&&card.dataset.theme+'-'+card.dataset.size!==value)}document.querySelectorAll('button').forEach(button=>button.onclick=()=>filter(button.dataset.filter));filter('light-desktop')</script></html>`);
console.log(`Saved ${records.length} screenshots and gallery to ${directory}`);
if(errors.length)throw new Error(errors.join('\n'));
