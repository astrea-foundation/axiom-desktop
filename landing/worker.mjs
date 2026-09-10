const accountRoutes = new Set([
  '/signin', '/signup', '/account', '/recover', '/reset-password', '/verify-email',
  '/native/authorize', '/payments/return', '/privacy', '/terms',
]);
const pages = new Map([['/', '/index.html'], ['/manifesto', '/manifesto.html'], ['/downloads', '/downloads.html']]);

function headers(initial) {
  const result = new Headers(initial);
  result.set('x-content-type-options', 'nosniff');
  result.set('referrer-policy', 'strict-origin-when-cross-origin');
  result.set('x-frame-options', 'SAMEORIGIN');
  result.set('permissions-policy', 'camera=(), microphone=(), geolocation=()');
  result.delete('set-cookie');
  return result;
}
function redirect(location) {
  return new Response(null, { status: 302, headers: headers({location, 'cache-control': 'no-store'}) });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (!['GET', 'HEAD'].includes(request.method)) {
      return new Response('Method Not Allowed', {status:405, headers:headers({allow:'GET, HEAD', 'cache-control':'no-store'})});
    }
    if (url.hostname === 'welcome.axiom.stream') {
      url.protocol = 'https:'; url.host = 'axiom.stream';
      return redirect(url.href);
    }
    const path = url.pathname.replace(/\/+$/, '') || '/';
    if (accountRoutes.has(path)) {
      return redirect(`https://auth.axiom.stream${path}${url.search}`);
    }
    const aliases = {'/index.html':'/', '/manifesto.html':'/manifesto', '/downloads.html':'/downloads', '/app':'/downloads'};
    if (aliases[path]) return redirect(`${aliases[path]}${url.search}`);
    if (pages.has(path) && url.pathname !== path) return redirect(`${path}${url.search}`);
    // Only public, build-validated release metadata is exposed here. No API proxy.
    const releaseMatch = /^\/api\/releases(?:\/([^/]+))?$/.exec(path);
    if (releaseMatch && (!releaseMatch[1] || releaseMatch[1] === 'latest' || VERSION_PATTERN.test(releaseMatch[1]))) {
      const assetUrl = new URL(`/releases/${releaseMatch[1] ?? 'index'}.json`, url);
      const asset = await env.ASSETS.fetch(new Request(assetUrl, {method: request.method}));
      const responseHeaders = headers(asset.headers);
      responseHeaders.set('content-type', 'application/json; charset=utf-8');
      responseHeaders.set('access-control-allow-origin', '*');
      responseHeaders.set('cache-control', asset.ok ? 'public, max-age=60, must-revalidate' : 'no-store');
      if (!asset.ok) responseHeaders.delete('content-length');
      return new Response(request.method === 'HEAD' ? null : asset.ok ? asset.body : JSON.stringify({error: 'Release not found'}), {
        status: asset.ok ? 200 : 404, headers: responseHeaders,
      });
    }
    // No SPA fallback, session state, or model requests on the marketing site.
    if (path.startsWith('/api/') || path.split('/').some(part => part.startsWith('.'))) {
      return new Response('Not Found', {status:404, headers:headers({'cache-control':'no-store'})});
    }
    const assetUrl = new URL(url);
    assetUrl.pathname = pages.get(path) ?? path;
    assetUrl.search = '';
    const asset = await env.ASSETS.fetch(new Request(assetUrl, {method:request.method}));
    if (asset.status === 404) {
      const missingUrl = new URL('/404.html', url);
      const missing = await env.ASSETS.fetch(new Request(missingUrl, {method:request.method}));
      return new Response(request.method === 'HEAD' ? null : missing.body, {
        status:404, headers:headers({'content-type':'text/html; charset=utf-8', 'cache-control':'no-store'}),
      });
    }
    const responseHeaders = headers(asset.headers);
    const html = responseHeaders.get('content-type')?.includes('text/html');
    const immutable = /^\/dist\/.+-[A-Z0-9]{8}\.(?:js|css)$/.test(path);
    responseHeaders.set('cache-control', html ? 'no-cache' : immutable ? 'public, max-age=31536000, immutable' : 'public, max-age=300, must-revalidate');
    return new Response(request.method === 'HEAD' ? null : asset.body, {status:asset.status, headers:responseHeaders});
  },
};
import {VERSION_PATTERN} from '../packages/desktop-releases/manifest.mjs';
