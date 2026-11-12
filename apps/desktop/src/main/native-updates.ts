import { spawn } from 'node:child_process';
import { createChildEnvironment, DESKTOP_SIDECAR_INHERITED_ENV } from '@axiom/axiom-acp-client';
import type { NativeUpdateEvent } from './update-service';

export function nativeUpdates(resolveExecutable: () => Promise<string>, spawnProcess: typeof spawn = spawn) {
  return async (args: string[], receive: (event: NativeUpdateEvent) => void, signal: AbortSignal): Promise<void> => {
    signal.throwIfAborted();
    const executable = await resolveExecutable();
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      const child = spawnProcess(executable, ['update', '--json', ...args], {
        windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], signal,
        env: createChildEnvironment(process.env,
          process.env.APPIMAGE ? {APPIMAGE: process.env.APPIMAGE} : {},
          ['AXIOM_API_KEY', 'AXIOM_PROXY_TOKEN'], DESKTOP_SIDECAR_INHERITED_ENV),
      });
      const handoff = args[0] === '--start-job';
      let pending = '', error = '', finished = false, acknowledged = false, exited = false;
      const releasePipes = () => { child.stdout.destroy(); child.stderr.destroy(); };
      const fail = (reason: unknown) => { if (finished) return; finished = true; releasePipes(); child.kill(); reject(reason); };
      const finishHandoff = () => {
        if (finished || !handoff || !acknowledged || !exited) return;
        if (pending.trim()) return fail(new Error('Native update did not complete'));
        // The detached Windows helper can retain inherited pipe handles while
        // waiting for Desktop to exit. Its authenticated acknowledgement and
        // the CLI's successful exit complete the handoff without waiting for EOF.
        finished = true; releasePipes(); resolve();
      };
      child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
      child.stdout.on('data', chunk => {
        if (finished) return;
        pending += chunk;
        if (Buffer.byteLength(pending) > 256 * 1024) return fail(new Error('Update response is too large'));
        for (;;) {
          const index = pending.indexOf('\n'); if (index < 0) break;
          const line = pending.slice(0, index); pending = pending.slice(index + 1);
          try {
            const event = JSON.parse(line) as NativeUpdateEvent;
            receive(event);
            if (handoff && event.event === 'installing') acknowledged = true;
          } catch (reason) { fail(reason); break; }
        }
        finishHandoff();
      });
      child.stderr.on('data', chunk => { error = (error + chunk).slice(-4096); });
      child.once('error', fail);
      child.once('exit', code => {
        if (!handoff || finished) return;
        if (code !== 0) return fail(new Error(error.trim() || 'Native update did not complete'));
        exited = true; finishHandoff();
      });
      child.once('close', code => {
        if (finished) return;
        if (code !== 0 || pending.trim() || (handoff && !acknowledged)) fail(new Error(error.trim() || 'Native update did not complete'));
        else { finished = true; resolve(); }
      });
    });
  };
}
