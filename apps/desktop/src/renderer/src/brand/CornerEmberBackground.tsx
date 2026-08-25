import { useEffect, useRef } from "react";
import { Mesh, Program, Renderer, Triangle } from "ogl";

interface CornerEmberBackgroundProps {
  pixelated?: boolean;
  peak?: number;
}

const vertex = /* glsl */ `
  attribute vec2 uv;
  attribute vec2 position;
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = vec4(position, 0.0, 1.0);
  }
`;

const fragment = /* glsl */ `
  precision highp float;
  uniform float uTime;
  uniform vec2 uResolution;
  uniform float uPixelated;
  uniform float uPeak;
  varying vec2 vUv;

  float hash(vec2 p) {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
  }

  float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(
      mix(hash(i + vec2(0.0, 0.0)), hash(i + vec2(1.0, 0.0)), u.x),
      mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x),
      u.y
    );
  }

  float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 5; i++) {
      v += a * noise(p);
      p = p * 2.0 + 13.0;
      a *= 0.5;
    }
    return v;
  }

  float bayer2(vec2 a) { a = floor(a); return fract(a.x / 2.0 + a.y * a.y * 0.75); }
  float bayer4(vec2 a) { return bayer2(0.5 * a) * 0.25 + bayer2(a); }

  void main() {
    float aspect = uResolution.x / max(uResolution.y, 1.0);
    vec2 uv = vUv;
    vec2 p = vec2(uv.x * aspect, uv.y);

    float t = uTime * 0.04;
    vec2 q = vec2(fbm(p * 2.2 + t), fbm(p * 2.2 + vec2(3.1, 1.7) - t));
    vec2 r = vec2(
      fbm(p * 2.2 + 1.6 * q + vec2(8.3, 2.8) + t * 1.1),
      fbm(p * 2.2 + 1.6 * q + vec2(1.2, 9.5) - t * 0.9)
    );
    float n = clamp(fbm(p * 2.2 + 2.0 * r) * 1.2, 0.0, 1.0);

    vec2 toCorner = vec2((1.0 - uv.x) * 0.8, 1.0 - uv.y);
    float d = length(toCorner);
    float mask = pow(1.0 - smoothstep(0.04, 1.0, d), 1.2);

    vec3 deep = vec3(0.541, 0.122, 0.188);
    vec3 accent = vec3(1.0, 0.302, 0.427);
    vec3 bright = vec3(1.0, 0.420, 0.522);
    float tone = clamp(n * 0.55 + mask * 0.6, 0.0, 1.0);
    vec3 col = mix(deep, accent, smoothstep(0.18, 0.5, tone));
    col = mix(col, bright, smoothstep(0.55, 0.92, tone));

    float energy = mask * (0.5 + 0.7 * n);
    float alpha = clamp(energy, 0.0, 1.0) * uPeak;

    if (uPixelated > 0.5) {
      col = floor(col * 5.0 + 0.5) / 5.0;
      float threshold = bayer4(gl_FragCoord.xy);
      float lit = step(threshold, alpha / max(uPeak, 0.001));
      alpha = lit * uPeak * step(0.05, mask);
    }

    gl_FragColor = vec4(col, alpha);
  }
`;

export function CornerEmberBackground({ pixelated = false, peak = 0.72 }: CornerEmberBackgroundProps) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const dpr = pixelated ? 0.14 : Math.min(window.devicePixelRatio || 1, 2);

    const renderer = new Renderer({ alpha: true, antialias: !pixelated, dpr });
    const gl = renderer.gl;
    gl.clearColor(0, 0, 0, 0);

    const canvas = gl.canvas as HTMLCanvasElement;
    canvas.style.width = "100%";
    canvas.style.height = "100%";
    canvas.style.display = "block";
    if (pixelated) canvas.style.imageRendering = "pixelated";
    container.appendChild(canvas);

    const program = new Program(gl, {
      vertex,
      fragment,
      transparent: true,
      uniforms: {
        uTime: { value: 0 },
        uResolution: { value: [1, 1] },
        uPixelated: { value: pixelated ? 1 : 0 },
        uPeak: { value: peak },
      },
    });
    program.setBlendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    const mesh = new Mesh(gl, { geometry: new Triangle(gl), program });

    let lastW = 0;
    let lastH = 0;
    const resize = () => {
      const w = container.clientWidth;
      const h = container.clientHeight;
      if (w === lastW && h === lastH) return;
      lastW = w;
      lastH = h;
      renderer.setSize(w, h);
      program.uniforms.uResolution.value = [gl.drawingBufferWidth, gl.drawingBufferHeight];
      renderer.render({ scene: mesh });
    };
    resize();
    const resizeObserver = new ResizeObserver(resize);
    resizeObserver.observe(container);

    let raf = 0;
    let running = false;
    const render = (time: number) => {
      program.uniforms.uTime.value = time * 0.001;
      renderer.render({ scene: mesh });
      raf = requestAnimationFrame(render);
    };
    const start = () => {
      if (running) return;
      running = true;
      raf = requestAnimationFrame(render);
    };
    const stop = () => {
      running = false;
      cancelAnimationFrame(raf);
    };

    const onVisibility = () => {
      if (document.hidden) stop();
      else start();
    };

    if (reduced) {
      program.uniforms.uTime.value = 14.0;
      renderer.render({ scene: mesh });
    } else {
      start();
      document.addEventListener("visibilitychange", onVisibility);
    }

    return () => {
      stop();
      resizeObserver.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
      if (canvas.parentNode) canvas.parentNode.removeChild(canvas);
    };
  }, [pixelated, peak]);

  return <div ref={containerRef} className="pointer-events-none absolute inset-0 overflow-hidden" />;
}
