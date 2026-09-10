// Local-only comparison harness. Run index.html?profile and use the sweep button.
// This module and its controls are excluded from the deployed site.
export function createProofProfile(renderer) {
  renderer.info.autoReset = false;
  const panel = document.createElement('aside');
  panel.style.cssText = 'position:fixed;bottom:12px;left:12px;z-index:99999;padding:12px;background:#fff;color:#111;font:12px/1.5 monospace;box-shadow:0 2px 12px #0002';
  const button = document.createElement('button');
  button.textContent = 'Run card performance sweep';
  const output = document.createElement('pre');
  output.id = 'proof-profile';
  panel.append(button, output);
  document.body.append(panel);
  let started = performance.now(), ready = 0, total = 0, lastFrame = 0;
  let sweeping = false, begin = 0, raf = 0, samples = [], calls = [], triangles = [];
  let work = [], frameStart = 0, result = null;
  const percentile = (values, fraction) => {
    const sorted = [...values].sort((a, b) => a - b);
    return +(sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))] || 0).toFixed(2);
  };
  const show = () => {
    output.textContent = JSON.stringify({ readyMs: ready, frames: total,
      canvas: [renderer.domElement.width, renderer.domElement.height],
      status: sweeping ? 'sweeping' : 'idle', result }, null, 2);
  };
  button.addEventListener('click', () => {
    cancelAnimationFrame(raf);
    samples = []; calls = []; triangles = []; work = []; result = null;
    begin = performance.now(); lastFrame = 0; sweeping = true;
    const track = document.querySelector('.proof-scroll');
    const stage = document.querySelector('.proof-stage');
    const top = track.getBoundingClientRect().top + scrollY;
    const span = track.offsetHeight - stage.clientHeight + stage.clientHeight * .6;
    const lead = stage.clientHeight * .6;
    const sweep = time => {
      const t = Math.min(1, (time - begin) / 12000);
      scrollTo(0, top - lead + (.08 + t * .8) * span);
      if (t < 1) { raf = requestAnimationFrame(sweep); return; }
      sweeping = false;
      result = { frames: samples.length, medianFrameMs: percentile(samples, .5),
        p95FrameMs: percentile(samples, .95), medianRenderMs: percentile(work, .5),
        medianDrawCalls: percentile(calls, .5), medianTriangles: percentile(triangles, .5) };
      show();
    };
    raf = requestAnimationFrame(sweep);
    show();
  });
  const refresh = setInterval(show, 1000);
  show();
  return {
    ready() { ready = Math.round(performance.now() - started); show(); },
    before() { renderer.info.reset(); frameStart = performance.now(); },
    after(time) {
      total++;
      if (sweeping && time - begin > 1000) {
        if (lastFrame) samples.push(time - lastFrame);
        work.push(performance.now() - frameStart);
        calls.push(renderer.info.render.calls);
        triangles.push(renderer.info.render.triangles);
      }
      lastFrame = time;
    },
    dispose() { clearInterval(refresh); cancelAnimationFrame(raf); panel.remove(); },
  };
}
