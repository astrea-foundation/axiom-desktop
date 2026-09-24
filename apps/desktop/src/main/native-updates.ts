import { spawn } from 'node:child_process';
import { createChildEnvironment, DESKTOP_SIDECAR_INHERITED_ENV } from '@axiom/axiom-acp-client';
import type { NativeUpdateEvent } from './update-service';

export function nativeUpdates(resolveExecutable: () => Promise<string>) {
  return async (args: string[], receive: (event: NativeUpdateEvent) => void, signal: AbortSignal): Promise<void> => {
    signal.throwIfAborted();
    const executable = await resolveExecutable();
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const child = spawn(executable, ['update', '--json', ...args], {
        windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], signal,
        env: createChildEnvironment(process.env,
          process.env.APPIMAGE ? {APPIMAGE: process.env.APPIMAGE} : {},
          ['AXIOM_API_KEY', 'AXIOM_PROXY_TOKEN'], DESKTOP_SIDECAR_INHERITED_ENV),
      });
      let pending = '', error = '', failed = false;
      const fail = (reason: unknown) => { if (failed) return; failed = true; child.kill(); reject(reason); };
      child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
      child.stdout.on('data', chunk => {
        if (failed) return;
        pending += chunk;
        if (Buffer.byteLength(pending) > 256 * 1024) return fail(new Error('Update response is too large'));
        for (;;) {
          const index = pending.indexOf('\n'); if (index < 0) break;
          const line = pending.slice(0, index); pending = pending.slice(index + 1);
          try { receive(JSON.parse(line) as NativeUpdateEvent); } catch (reason) { fail(reason); break; }
        }
      });
      child.stderr.on('data', chunk => { error = (error + chunk).slice(-4096); });
      child.once('error', fail);
      child.once('close', code => {
        if (failed) return;
        if (code !== 0 || pending.trim()) reject(new Error(error.trim() || 'Native update did not complete'));
        else resolve();
      });
    });
  };
}
