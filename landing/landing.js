import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { EffectComposer } from 'three/addons/postprocessing/EffectComposer.js';
import { RenderPass } from 'three/addons/postprocessing/RenderPass.js';
import { BokehPass } from 'three/addons/postprocessing/BokehPass.js';
import { OutputPass } from 'three/addons/postprocessing/OutputPass.js';

/* ============================================================
   Fabric textures — ported from holocloth (textures.ts):
   procedural plain-weave normal map + grain roughness map.
   ============================================================ */

function mulberry32(seed) {
  return () => {
    seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function weaveHeight(size, threads, rand) {
  const h = new Float32Array(size * size);
  const warpVar = new Float32Array(threads + 1);
  const weftVar = new Float32Array(threads + 1);
  for (let i = 0; i <= threads; i++) {
    warpVar[i] = 0.85 + rand() * 0.3;
    weftVar[i] = 0.85 + rand() * 0.3;
  }
  const grain = new Float32Array(size * size);
  for (let i = 0; i < grain.length; i++) grain[i] = rand();
  for (let y = 0; y < size; y++) {
    const v = (y / size) * threads;
    const vi = Math.floor(v), vf = v - vi;
    for (let x = 0; x < size; x++) {
      const u = (x / size) * threads;
      const ui = Math.floor(u), uf = u - ui;
      const warp = Math.sin(uf * Math.PI) * warpVar[ui % (threads + 1)];
      const weft = Math.sin(vf * Math.PI) * weftVar[vi % (threads + 1)];
      const over = (ui + vi) % 2 === 0;
      const height = over ? warp * 0.62 + weft * 0.38 : weft * 0.62 + warp * 0.38;
      h[y * size + x] = height * 0.85 + grain[y * size + x] * 0.15;
    }
  }
  return h;
}

function heightToNormalTexture(h, size, strength) {
  const canvas = document.createElement('canvas');
  canvas.width = size; canvas.height = size;
  const ctx = canvas.getContext('2d');
  const img = ctx.createImageData(size, size);
  const data = img.data;
  const at = (x, y) => h[((y + size) % size) * size + ((x + size) % size)];
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = (at(x + 1, y) - at(x - 1, y)) * strength;
      const dy = (at(x, y + 1) - at(x, y - 1)) * strength;
      const inv = 1 / Math.sqrt(dx * dx + dy * dy + 1);
      const i = (y * size + x) * 4;
      data[i] = Math.round((-dx * inv * 0.5 + 0.5) * 255);
      data[i + 1] = Math.round((dy * inv * 0.5 + 0.5) * 255);
      data[i + 2] = Math.round((inv * 0.5 + 0.5) * 255);
      data[i + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  const tex = new THREE.CanvasTexture(canvas);
  tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
  tex.colorSpace = THREE.NoColorSpace;
  return tex;
}

const makeWeaveNormalMap = (size = 512, threads = 64, seed = 1337) =>
  heightToNormalTexture(weaveHeight(size, threads, mulberry32(seed)), size, 1.6);

function makeGrainRoughnessMap(size = 256, seed = 4242) {
  const rand = mulberry32(seed);
  const canvas = document.createElement('canvas');
  canvas.width = size; canvas.height = size;
  const ctx = canvas.getContext('2d');
  const img = ctx.createImageData(size, size);
  for (let i = 0; i < size * size; i++) {
    const v = Math.round(215 + (rand() - 0.5) * 70);
    img.data[i * 4] = v; img.data[i * 4 + 1] = v; img.data[i * 4 + 2] = v; img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  const tex = new THREE.CanvasTexture(canvas);
  tex.wrapS = tex.wrapT = THREE.RepeatWrapping;
  tex.colorSpace = THREE.NoColorSpace;
  return tex;
}

/* ============================================================
   Cloth simulation — ported from holocloth (Verlet,
   structural/shear/bend 1.0/0.85/0.35, laplacian smoothing,
   viscosity damping, 1/120 substeps), extended with pins,
   wind, and gravity.
   ============================================================ */

const PHYS = {
  viscosity: 0.36,
  stiffness: 1.0,
  iterations: 9,
  smoothing: 0.045,
};
const WIND = {
  dir: new THREE.Vector3(-0.88, -0.42, 0.10).normalize(),
  base: 1.15,
  gust: 1.0,
  flutter: 4.6,
  gravity: -0.35,
  endSway: 2.4,
};
const SUBSTEP = 1 / 120;
const MAX_SUBSTEPS = 4;

class BannerSim {
  constructor(length, width, segX, segY, initFn) {
    this.length = length; this.width = width;
    this.segX = segX; this.segY = segY;
    this.cols = segX + 1; this.rows = segY + 1;
    this.count = this.cols * this.rows;
    this.positions = new Float32Array(this.count * 3);
    this.prev = new Float32Array(this.count * 3);

    let k = 0;
    for (let y = 0; y < this.rows; y++) {
      for (let x = 0; x < this.cols; x++) {
        const p = initFn(x / this.segX, y / this.segY);
        this.positions[k] = p[0]; this.positions[k + 1] = p[1]; this.positions[k + 2] = p[2];
        k += 3;
      }
    }
    this.prev.set(this.positions);

    // the banner is attached out of view, top right: pin the last two columns
    this.pins = [];
    for (let y = 0; y < this.rows; y++) {
      for (const x of [this.segX - 1, this.segX]) {
        const i = y * this.cols + x;
        this.pins.push(i, this.positions[i * 3], this.positions[i * 3 + 1], this.positions[i * 3 + 2]);
      }
    }

    const a = [], b = [], mul = [];
    const idx = (x, y) => y * this.cols + x;
    for (let y = 0; y < this.rows; y++) {
      for (let x = 0; x < this.cols; x++) {
        if (x + 1 < this.cols) { a.push(idx(x, y)); b.push(idx(x + 1, y)); mul.push(1.0); }
        if (y + 1 < this.rows) { a.push(idx(x, y)); b.push(idx(x, y + 1)); mul.push(1.0); }
        if (x + 1 < this.cols && y + 1 < this.rows) {
          a.push(idx(x, y)); b.push(idx(x + 1, y + 1)); mul.push(0.85);
          a.push(idx(x + 1, y)); b.push(idx(x, y + 1)); mul.push(0.85);
        }
        if (x + 2 < this.cols) { a.push(idx(x, y)); b.push(idx(x + 2, y)); mul.push(0.35); }
        if (y + 2 < this.rows) { a.push(idx(x, y)); b.push(idx(x, y + 2)); mul.push(0.35); }
      }
    }
    this.cA = new Int32Array(a); this.cB = new Int32Array(b); this.cMul = new Float32Array(mul);
    this.cRest = new Float32Array(a.length);
    // rest lengths from the authored pose: the S-sweep IS the banner's
    // natural shape; the wind moves it around that state
    const p0 = this.positions;
    for (let c = 0; c < a.length; c++) {
      const ia = this.cA[c] * 3, ib = this.cB[c] * 3;
      this.cRest[c] = Math.hypot(p0[ib] - p0[ia], p0[ib + 1] - p0[ia + 1], p0[ib + 2] - p0[ia + 2]);
    }

    this.neighbors = new Int32Array(this.count * 4).fill(-1);
    for (let y = 0; y < this.rows; y++) {
      for (let x = 0; x < this.cols; x++) {
        const i = idx(x, y) * 4;
        this.neighbors[i] = x > 0 ? idx(x - 1, y) : -1;
        this.neighbors[i + 1] = x + 1 < this.cols ? idx(x + 1, y) : -1;
        this.neighbors[i + 2] = y > 0 ? idx(x, y - 1) : -1;
        this.neighbors[i + 3] = y + 1 < this.rows ? idx(x, y + 1) : -1;
      }
    }
    this.accumulator = 0;
    this.time = 0;
    this.airX = 0; this.airY = 0;   // where the cloth sits within the air's wave field
    this.released = false;   // pins let go; only the fall bake sets this
  }

  /** Hover ripples: each cursor movement spawns a short-lived expanding
      ring of force that presses the fabric gently away from the viewer.
      Force-based like the wind, so it decays and can never accumulate. */
  addRipple(x, y, amp) {
    if (!this.ripples) this.ripples = [];
    if (this.ripples.length > 7) this.ripples.shift();
    this.ripples.push({ x, y, age: 0, amp });
  }

  step(dt) {
    this.accumulator += Math.min(dt, 0.05);
    const cap = this.maxSub || MAX_SUBSTEPS;
    let steps = 0;
    while (this.accumulator >= SUBSTEP && steps < cap) {
      this.substep();
      this.accumulator -= SUBSTEP;
      steps++;
    }
    if (steps === cap) this.accumulator = 0;
  }

  substep() {
    const p = this.positions, prev = this.prev, n = this.count;
    const t = (this.time += SUBSTEP);
    if (this.ripples && this.ripples.length) {
      for (let rI = 0; rI < this.ripples.length; rI++) this.ripples[rI].age += SUBSTEP;
      this.ripples = this.ripples.filter((rp) => rp.age < 1.4);
    }
    const dt2 = SUBSTEP * SUBSTEP;

    const visc = this.released ? PHYS.viscosity * 0.16 : PHYS.viscosity;
    const damp = Math.pow(1 - Math.min(visc, 0.99), SUBSTEP * 60);
    const wm = this.windMul === undefined ? 1 : this.windMul;  // per-banner breeze
    const wb = (1 + (this.windBoost || 0) * 1.9) * wm;
    const gustF = (WIND.base + WIND.gust * (0.6 * Math.sin(t * 0.45) + 0.4 * Math.sin(t * 0.19 + 2.0))) * wb;
    const d = WIND.dir;
    for (let i = 0; i < n; i++) {
      const col = i % this.cols;
      const u = 1 - col / this.segX;
      const amp = 0.55 + 0.45 * Math.pow(u, 0.9);   // present along the whole banner
      // the wave field belongs to the AIR, not the fabric: phases are keyed
      // to world position, so when the banner falls it visibly navigates
      // through waves that stay where the wind put them
      const wxp = p[i * 3] + this.airX;
      const wyp = p[i * 3 + 1] + this.airY;
      const s1 = wxp * 0.79 + wyp * 0.61;     // along the sweep
      const s2 = -wxp * 0.61 + wyp * 0.79;    // across it
      const flut =
        WIND.flutter * wb * amp * (Math.sin(s1 * 6.0 + t * 2.3 + s2 * 1.7) * 0.7 +
                                   Math.sin(s1 * 1.9 + t * 0.9) * 0.45);
      const row = (i / this.cols) | 0;
      const endSway = WIND.endSway * wm * Math.pow(u, 2.6) * Math.sin(t * 0.65 + row * 0.1);
      let ax = d.x * gustF * amp;
      let ay = d.y * gustF * amp + WIND.gravity * amp * 0.8 + endSway;
      let az = d.z * gustF * amp + flut;
      if (this.released) {
        // freefall: strong gravity, a leftward stream that pulls the free
        // end hardest so the banner tumbles tail-first toward the corner,
        // and extra depth churn so it billows on the way down
        ax += -0.9 - 1.3 * u;
        ay += -3.0;
        az += amp * 1.3 * Math.sin(s1 * 2.6 + t * 1.4);
      }
      if (this.ripples) {
        for (let rI = 0; rI < this.ripples.length; rI++) {
          const rp = this.ripples[rI];
          const rdx = p[i * 3] - rp.x, rdy = p[i * 3 + 1] - rp.y;
          const rd = Math.sqrt(rdx * rdx + rdy * rdy);
          const ring = rd - (0.12 + 1.1 * rp.age);
          const wgt = Math.exp(-ring * ring * 22) * Math.exp(-2.6 * rp.age);
          // push-then-pull: the ripple's net impulse is zero, so however
          // hard the cursor stirs, it can never reel the cloth anywhere
          az -= rp.amp * wgt * Math.cos(rp.age * 5.0) * Math.max(0.5, amp);
        }
      }
      for (let c = 0; c < 3; c++) {
        const j = i * 3 + c;
        const cur = p[j];
        const vel = (cur - prev[j]) * damp;
        prev[j] = cur;
        p[j] = cur + vel + (c === 0 ? ax : c === 1 ? ay : az) * dt2;
      }
    }

    if (PHYS.smoothing > 0) {
      const k = PHYS.smoothing * 0.5, nb = this.neighbors;
      for (let i = 0; i < n; i++) {
        let axx = 0, ayy = 0, azz = 0, cnt = 0;
        for (let j = 0; j < 4; j++) {
          const ni = nb[i * 4 + j];
          if (ni < 0) continue;
          axx += p[ni * 3]; ayy += p[ni * 3 + 1]; azz += p[ni * 3 + 2]; cnt++;
        }
        if (!cnt) continue;
        const inv = 1 / cnt;
        p[i * 3] += (axx * inv - p[i * 3]) * k;
        p[i * 3 + 1] += (ayy * inv - p[i * 3 + 1]) * k;
        p[i * 3 + 2] += (azz * inv - p[i * 3 + 2]) * k;
      }
    }

    const iters = PHYS.iterations, stiff = PHYS.stiffness;
    const cA = this.cA, cB = this.cB, cRest = this.cRest, cMul = this.cMul, nc = cA.length;
    for (let it = 0; it < iters; it++) {
      for (let c = 0; c < nc; c++) {
        const ia = cA[c] * 3, ib = cB[c] * 3;
        const dx = p[ib] - p[ia], dy = p[ib + 1] - p[ia + 1], dz = p[ib + 2] - p[ia + 2];
        const dist = Math.sqrt(dx * dx + dy * dy + dz * dz);
        if (dist < 1e-9) continue;
        const diff = ((dist - cRest[c]) / dist) * 0.5 * stiff * cMul[c];
        const ox = dx * diff, oy = dy * diff, oz = dz * diff;
        p[ia] += ox; p[ia + 1] += oy; p[ia + 2] += oz;
        p[ib] -= ox; p[ib + 1] -= oy; p[ib + 2] -= oz;
      }
      this.applyPins();
    }
  }

  applyPins() {
    if (this.released) return;
    const p = this.positions, pins = this.pins;
    const pw = this.pinWave;
    if (pw) {
      // the held edge floats: each pinned vertex orbits its anchor slowly,
      // phased along the edge, so the top of the cloth breathes with the air
      const t = this.time;
      for (let k = 0; k < pins.length; k += 4) {
        const i = pins[k] * 3;
        const ph = pins[k + 1] * 2.6 + pins[k + 2] * 1.3;
        p[i] = pins[k + 1] + pw * 0.5 * Math.sin(t * 0.5 + ph);
        p[i + 1] = pins[k + 2] + pw * 0.35 * Math.sin(t * 0.7 + ph * 1.4);
        p[i + 2] = pins[k + 3] + pw * Math.sin(t * 0.85 + ph);
      }
      return;
    }
    for (let k = 0; k < pins.length; k += 4) {
      const i = pins[k] * 3;
      p[i] = pins[k + 1]; p[i + 1] = pins[k + 2]; p[i + 2] = pins[k + 3];
    }
  }

  computeCavity(normals, out, gain = 6) {
    const p = this.positions, nb = this.neighbors, n = this.count;
    const invStep = 1 / Math.min(this.length / this.segX, this.width / this.segY);
    if (!this._tmp || this._tmp.length < n) this._tmp = new Float32Array(n);
    const tmp = this._tmp;
    for (let i = 0; i < n; i++) {
      let ax = 0, ay = 0, az = 0, cnt = 0;
      for (let j = 0; j < 4; j++) {
        const ni = nb[i * 4 + j];
        if (ni < 0) continue;
        ax += p[ni * 3]; ay += p[ni * 3 + 1]; az += p[ni * 3 + 2]; cnt++;
      }
      if (!cnt) { tmp[i] = 0; continue; }
      const inv = 1 / cnt;
      const lx = ax * inv - p[i * 3], ly = ay * inv - p[i * 3 + 1], lz = az * inv - p[i * 3 + 2];
      const c = (lx * normals[i * 3] + ly * normals[i * 3 + 1] + lz * normals[i * 3 + 2]) * invStep;
      tmp[i] = Math.min(1, Math.max(0, c * gain));
    }
    for (let i = 0; i < n; i++) {
      let sum = 0, cnt = 0;
      for (let j = 0; j < 4; j++) {
        const ni = nb[i * 4 + j];
        if (ni < 0) continue;
        sum += tmp[ni]; cnt++;
      }
      out[i] = cnt ? tmp[i] * 0.5 + (sum / cnt) * 0.5 : tmp[i];
    }
  }
}

/* ============================================================
   Scene
   ============================================================ */

const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches
  || location.search.indexOf('static') !== -1;

/* barely-there smooth scrolling: wheel input is eased over a few frames
   so mouse steps stop jolting; touch, keys and the scrollbar stay native */
if (!reduced) {
  let target = window.scrollY;
  let cur = target;
  let lastSet = -1;
  let settling = false;
  let glideFrame = 0;
  const maxScroll = () => document.documentElement.scrollHeight - window.innerHeight;
  addEventListener('wheel', (ev) => {
    if (ev.ctrlKey) return;                   // pinch-zoom stays native
    ev.preventDefault();
    const dy = ev.deltaMode === 1 ? ev.deltaY * 16 : ev.deltaY;
    if (!settling) { cur = target = window.scrollY; lastSet = -1; }
    target = Math.max(0, Math.min(maxScroll(), target + dy));
    settling = true;
    if (!glideFrame) glideFrame = requestAnimationFrame(glide);
  }, { passive: false });
  function glide() {
    glideFrame = 0;
    if (settling) {
      if (lastSet >= 0 && Math.abs(window.scrollY - lastSet) > 2) {
        cur = target = window.scrollY;        // something else scrolled; follow it
        settling = false;
      } else {
        cur += (target - cur) * 0.16;
        if (Math.abs(target - cur) < 0.5) { cur = target; settling = false; }
        window.scrollTo(0, cur);
        lastSet = Math.round(cur);
      }
    } else {
      lastSet = -1;
    }
    if (settling) glideFrame = requestAnimationFrame(glide);
  }
}

const canvas = document.getElementById('scene');
let renderer;
try {
  renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
} catch (e) {
  console.warn('WebGL unavailable; hero stays typographic.', e);
}

if (renderer) {
  renderer.setPixelRatio(Math.min(devicePixelRatio, 1.5));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.NoToneMapping;

  const scene = new THREE.Scene();
  scene.background = new THREE.Color('#F2F4FA');
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 20);
  camera.position.z = 6;

  // image-based lighting from a neutral studio room (holocloth)
  const pmrem = new THREE.PMREMGenerator(renderer);
  scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
  pmrem.dispose();

  // holocloth light rig geometry, rims tinted to brand warm orange
  const rimA = new THREE.DirectionalLight(0x6FA0FF, 0.85);
  rimA.position.set(-4, 2.5, -3);
  const rimB = new THREE.DirectionalLight(0x3A7CFF, 0.65);
  rimB.position.set(4.5, -1.5, -2.5);
  const key = new THREE.DirectionalLight(0xffffff, 0.7);
  key.position.set(1.5, 3, 4);
  scene.add(rimA, rimB, key);

  /* depth of field for the key reveal: the tip of the key, tilted toward
     the viewer, holds focus while the bird recedes into blur. The composer
     only runs while the reveal is on screen; the hero stays single-pass. */
  const DOF = { focus: 4.95, aperture: 0.055, maxblur: 0.038, on: 0.55, ramp: 0.3 };
  const composer = new EffectComposer(renderer);
  composer.setPixelRatio(Math.min(devicePixelRatio, 1.5));
  composer.addPass(new RenderPass(scene, camera));
  const bokehPass = new BokehPass(scene, camera, {
    focus: DOF.focus, aperture: DOF.aperture, maxblur: DOF.maxblur,
  });
  bokehPass.materialBokeh.defines.PERSPECTIVE_CAMERA = 0;  // ortho depth
  bokehPass.materialBokeh.needsUpdate = true;
  composer.addPass(bokehPass);
  composer.addPass(new OutputPass());

  const root = new THREE.Group();
  scene.add(root);

  /* ---------- the banner ---------- */
  const curve = new THREE.CatmullRomCurve3([
    new THREE.Vector3(-1.30, -1.02, 0.06),   // free end, lower left
    new THREE.Vector3(-0.78, -0.50, -0.02),
    new THREE.Vector3(-0.05, -0.06, 0.04),
    new THREE.Vector3(0.62, 0.14, -0.04),    // the S waist
    new THREE.Vector3(1.28, 0.66, 0.02),
    new THREE.Vector3(2.15, 1.38, 0.0),      // pinned, out of view top right
  ]);
  const BANNER_LEN = curve.getLength();
  const BANNER_W = 1.00;
  const SEG_X = 96, SEG_Y = 26;

  const _pt = new THREE.Vector3(), _tan = new THREE.Vector3();
  const NOTCH = 0.085;                        // swallowtail depth, fraction of length
  const R_EPS = 0.012;                        // softens the notch apex by a few px
  const sim = new BannerSim(BANNER_LEN, BANNER_W, SEG_X, SEG_Y, (u, v) => {
    // the free end is cut into a swallowtail: middle rows start deeper in,
    // so the end edge traces a V. smooth-abs rounds the apex; the two tail
    // tips get a small chamfer by sampling a step toward the interior
    if (u === 0 && (v === 0 || v === 1)) {
      u += 0.35 / SEG_X;
      v = v === 0 ? 0.35 / SEG_Y : 1 - 0.35 / SEG_Y;
    }
    const dAbs = Math.sqrt((v - 0.5) * (v - 0.5) + R_EPS * R_EPS) - R_EPS;
    const uMin = Math.max(0, NOTCH * (1 - 2 * dAbs));
    const uMax = 1 - uMin;                    // matching V at the hidden end,
    u = uMin + (uMax - uMin) * u;             // revealed when the banner departs
    curve.getPointAt(u, _pt);
    curve.getTangentAt(u, _tan);
    const nx = -_tan.y, ny = _tan.x;
    const off = (v - 0.5) * BANNER_W;
    return [
      _pt.x + nx * off,
      _pt.y + ny * off,
      _pt.z + 0.02 * Math.sin(u * 9.0) + 0.015 * Math.sin(v * 6.2),
    ];
  });

  const clothGeo = new THREE.PlaneGeometry(1, 1, SEG_X, SEG_Y);
  const colors = new Float32Array(sim.count * 3).fill(1);
  clothGeo.setAttribute('color', new THREE.BufferAttribute(colors, 3));

  // fabric material: brand orange + holocloth weave bump + grain roughness
  const maxAniso = renderer.capabilities.getMaxAnisotropy();
  // the full jacquard PBR stack, Blender-style: true normal map and
  // roughness map from the material set, plus fabric sheen
  const clothMat = new THREE.MeshPhysicalMaterial({
    color: new THREE.Color('#3A7CFF'),
    metalness: 0,
    roughness: 1.0,                           // the map carries the values
    normalScale: new THREE.Vector2(0.9, 0.9),
    sheen: 0.45,                              // fabric glow at grazing angles
    sheenRoughness: 0.5,
    sheenColor: new THREE.Color('#A9C4FF'),
    envMapIntensity: 0.78,
    vertexColors: true,                       // cavity AO lives in vertex colors
    side: THREE.DoubleSide,
  });
  const cloth = new THREE.Mesh(clothGeo, clothMat);
  cloth.frustumCulled = false;
  root.add(cloth);

  // cloth surface: brand blue with the floral jacquard woven in
  // tone-on-tone (color) and as sheen contrast (roughness), plus the
  // banner print on top. figure glossier than ground, like real damask.
  const artImg = new Image();
  const patImg = new Image();
  const norImg = new Image();
  const roughImg = new Image();
  const flcPatImg = new Image();
  const flcNorImg = new Image();
  const flcRoughImg = new Image();
  let readyCount = 0;
  const TILE_W = 3140, TILE_H = 3140;         // jacquard tile size on the maps
  const FLEECE_TILE = 1150;                   // knit tile size on the maps
  const BORDER_WORLD = 0.095;                 // fleece selvage, ~10% of strip width
  const GAP_WORLD = 0.012;                    // thin flat parting line between them

  function insets(w, h, worldInset) {
    return [
      Math.round(worldInset / BANNER_LEN * w),
      Math.round(worldInset / BANNER_W * h),
    ];
  }

  /* zones: fleece everywhere -> flat fill from BORDER inward (leaves the
     fleece as a band and creates the flat gap ring) -> jacquard clipped
     from BORDER+GAP inward */
  function composeZones(cx, w, h, drawFleece, flatFill, drawField) {
    drawFleece();
    const [bu, bv] = insets(w, h, BORDER_WORLD);
    cx.save();
    cx.beginPath();
    cx.rect(bu, bv, w - 2 * bu, h - 2 * bv);
    cx.clip();
    flatFill();
    cx.restore();
    const [fu, fv] = insets(w, h, BORDER_WORLD + GAP_WORLD);
    cx.save();
    cx.beginPath();
    cx.rect(fu, fv, w - 2 * fu, h - 2 * fv);
    cx.clip();
    drawField();
    cx.restore();
  }

  function tileAcross(cx, img, w, h, tw, th) {
    for (let y = 0; y < h; y += th) {
      for (let x = 0; x < w; x += tw) cx.drawImage(img, x, y, tw, th);
    }
  }

  const VW = 4400, VH = 1000;               // virtual map space; canvases scale down
  function composeClothMaps() {
    if (readyCount < 7) return;

    // color: highest resolution of the three, it carries the print
    const c = document.createElement('canvas');
    c.width = 3200; c.height = 727;
    const cx = c.getContext('2d');
    cx.scale(c.width / VW, c.height / VH);
    cx.fillStyle = '#3A7CFF';
    cx.fillRect(0, 0, VW, VH);
    composeZones(cx, VW, VH,
      () => {
        cx.globalAlpha = 0.15;
        cx.globalCompositeOperation = 'soft-light';
        tileAcross(cx, flcPatImg, VW, VH, FLEECE_TILE, FLEECE_TILE);
        cx.globalAlpha = 1;
        cx.globalCompositeOperation = 'source-over';
      },
      () => { cx.fillStyle = '#3A7CFF'; cx.fillRect(0, 0, VW, VH); },
      () => {
        cx.globalAlpha = 0.18;
        cx.globalCompositeOperation = 'soft-light';
        cx.filter = 'contrast(1.15)';
        tileAcross(cx, patImg, VW, VH, TILE_W, TILE_H);
        cx.filter = 'none';
        cx.globalAlpha = 1;
        cx.globalCompositeOperation = 'source-over';
      });
    cx.translate(0, VH);                      // ribbon UVs are v-flipped
    cx.scale(1, -1);                          // relative to the art
    cx.drawImage(artImg, 0, 0, VW, VH);
    const tex = new THREE.CanvasTexture(c);
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.anisotropy = 8;
    clothMat.map = tex;
    clothMat.color.set('#FFFFFF');

    // normals: relief doesn't need print resolution
    const nc = document.createElement('canvas');
    nc.width = 2048; nc.height = 466;
    const nx = nc.getContext('2d');
    nx.scale(nc.width / VW, nc.height / VH);
    nx.fillStyle = 'rgb(128, 128, 255)';
    nx.fillRect(0, 0, VW, VH);
    composeZones(nx, VW, VH,
      () => tileAcross(nx, flcNorImg, VW, VH, FLEECE_TILE, FLEECE_TILE),
      () => { nx.fillStyle = 'rgb(128, 128, 255)'; nx.fillRect(0, 0, VW, VH); },
      () => tileAcross(nx, norImg, VW, VH, TILE_W, TILE_H));
    const ntex = new THREE.CanvasTexture(nc);
    ntex.colorSpace = THREE.NoColorSpace;
    ntex.anisotropy = 4;
    clothMat.normalMap = ntex;

    // roughness: broad strokes are enough
    const rc = document.createElement('canvas');
    rc.width = 1400; rc.height = 318;
    const rx = rc.getContext('2d');
    rx.scale(rc.width / VW, rc.height / VH);
    rx.fillStyle = 'rgb(185, 185, 185)';
    rx.fillRect(0, 0, VW, VH);
    composeZones(rx, VW, VH,
      () => tileAcross(rx, flcRoughImg, VW, VH, FLEECE_TILE, FLEECE_TILE),
      () => { rx.fillStyle = 'rgb(185, 185, 185)'; rx.fillRect(0, 0, VW, VH); },
      () => tileAcross(rx, roughImg, VW, VH, TILE_W, TILE_H));
    const rtex = new THREE.CanvasTexture(rc);
    rtex.colorSpace = THREE.NoColorSpace;
    rtex.anisotropy = 4;
    clothMat.roughnessMap = rtex;

    clothMat.needsUpdate = true;
  }
  const onReady = () => { readyCount++; composeClothMaps(); composePennantMaps(); };
  for (const im of [artImg, patImg, norImg, roughImg, flcPatImg, flcNorImg, flcRoughImg]) im.onload = onReady;
  artImg.src = 'assets/banner-overlay.png?v=3';
  patImg.src = 'assets/jacquard-pattern.jpg';
  norImg.src = 'assets/jacquard-nor.webp';
  roughImg.src = 'assets/jacquard-rough.webp';
  flcPatImg.src = 'assets/fleece-pattern.jpg';
  flcNorImg.src = 'assets/fleece-nor.webp';
  flcRoughImg.src = 'assets/fleece-rough.webp';


  const cavity = new Float32Array(sim.count);
  let syncTick = 0;
  let SYNC_EVERY = 2;      // normals/cavity cadence; the quality tiers move it
  function syncCloth() {
    const pos = clothGeo.attributes.position;
    pos.array.set(sim.positions);
    pos.needsUpdate = true;
    // normals, cavity and shading refresh at a reduced rate; invisible at
    // this motion speed, and it carries most of the per-frame geometry cost
    if (syncTick++ % SYNC_EVERY === 0) {
      clothGeo.computeVertexNormals();
      sim.computeCavity(clothGeo.attributes.normal.array, cavity, 6);
      const col = clothGeo.attributes.color;
      for (let i = 0; i < sim.count; i++) {
        const shade = 1 - 0.17 * cavity[i];
        col.array[i * 3] = shade; col.array[i * 3 + 1] = shade; col.array[i * 3 + 2] = shade;
      }
      col.needsUpdate = true;
      clothGeo.attributes.normal.needsUpdate = true;
    }
  }

  /* ---------- the falcon and key ---------- */
  const modelGroup = new THREE.Group();
  root.add(modelGroup);
  const MODEL = { scale: 2.95, x: 0.0, y: -0.88, z: 0.9, rx: 0.0, ry: -0.45, rz: 0.0 };

  const loader = new GLTFLoader();
  loader.setMeshoptDecoder(MeshoptDecoder);
  loader.load('assets/falcon-key.glb', (gltf) => {
    const obj = gltf.scene;
    const box = new THREE.Box3().setFromObject(obj);
    obj.position.sub(box.getCenter(new THREE.Vector3()));
    const inner = new THREE.Group();
    inner.add(obj);
    inner.scale.setScalar(MODEL.scale);
    inner.rotation.order = 'ZYX';             // tilt applied last, stays in-plane
    inner.rotation.set(MODEL.rx, MODEL.ry, MODEL.rz);
    innerRef = inner;
    modelGroup.add(inner);
    modelGroup.position.set(MODEL.x, MODEL.y, MODEL.z);
    obj.traverse((c) => {
      if (c.isMesh && c.material) {
        c.material.envMapIntensity = 0.9;
        c.material.needsUpdate = true;
      }
    });
    canvas.classList.add('ready');
    if (reduced) renderer.render(scene, camera); // static mode: show the model too
  }, undefined, (err) => {
    console.warn('Model failed to load; banner only.', err);
    canvas.classList.add('ready');
  });

  /* ---------- pointer: hover ripples on the cloth ---------- */
  let innerRef = null;
  const pointer = { x: 0, y: 0, px: 0, py: 0, has: false };

  addEventListener('pointermove', (e) => {
    if (reduced) return;
    const aspect = innerWidth / innerHeight;
    const wx = ((e.clientX / innerWidth) * 2 - 1) * aspect / rootScale;
    const wy = (1 - (e.clientY / innerHeight) * 2) / rootScale;
    if (!pointer.has) { pointer.px = wx; pointer.py = wy; }
    pointer.x = wx; pointer.y = wy; pointer.has = true;
  }, { passive: true });

  let lastRipple = 0;
  function applyPointer(now) {
    if (!pointer.has || reduced || scrollHeroP > 0.05) return;
    const vx = pointer.x - pointer.px, vy = pointer.y - pointer.py;
    const speed = Math.hypot(vx, vy);
    if (speed > 0.004 && now - lastRipple > 90) {
      sim.addRipple(pointer.x, pointer.y, Math.min(5.0, speed * 85));
      lastRipple = now;
    }
    pointer.px = pointer.x; pointer.py = pointer.y;
  }

  /* ---------- sizing ---------- */
  let rootScale = 1;
  function resize() {
    const w = innerWidth, h = innerHeight;
    renderer.setSize(w, h, false);
    composer.setSize(w, h);
    const aspect = w / h;
    if (bokehPass.uniforms.aspect) bokehPass.uniforms.aspect.value = aspect;
    camera.left = -aspect; camera.right = aspect;
    camera.top = 1; camera.bottom = -1;
    camera.updateProjectionMatrix();
    rootScale = aspect < 0.8
      ? Math.max(0.45, aspect * 0.95)
      : Math.max(0.62, Math.min(1, aspect / 1.35));
    root.scale.setScalar(rootScale);
    // portrait: the scene (bird and cloth together) slides down so the
    // falcon's head always clears the text block, at any height
    let shift = 0;
    if (aspect < 0.9) {
      const cta = document.querySelector('.cta-mask') || document.querySelector('.cta');
      const textBottom = (cta ? cta.getBoundingClientRect().bottom + window.scrollY : h * 0.38) + 28;
      const birdTopWorld = (MODEL.y + (1.18 * MODEL.scale) / 2) * rootScale;
      shift = Math.min(0, 1 - (2 * textBottom) / h - birdTopWorld);
    }
    root.position.y = shift;
    if (reduced) renderer.render(scene, camera);  // loop is parked in static mode
  }
  addEventListener('resize', resize);
  resize();
  setTimeout(resize, 900);                    // re-measure once fonts settle

  /* ---------- adaptive quality: yield fidelity before yielding smoothness.
     The page watches its own frame times; on a slow device, or when other
     work is loading the machine, it steps down through tiers (resolution,
     solver iterations, shading cadence, substeps, depth of field) and steps
     back up once the machine has been comfortable for a while. ---------- */
  const QT = [
    { dpr: 1.5,  iters: 9, syncEvery: 2, maxSub: 4, dof: true  },
    { dpr: 1.2,  iters: 7, syncEvery: 3, maxSub: 3, dof: true  },
    { dpr: 1.0,  iters: 5, syncEvery: 4, maxSub: 2, dof: false },
  ];
  const QUALITY = { tier: 0, lastChange: 0 };
  let perfAvg = 16.7;
  function applyQuality(n) {
    QUALITY.tier = Math.max(0, Math.min(QT.length - 1, n));
    const q = QT[QUALITY.tier];
    PHYS.iterations = q.iters;
    SYNC_EVERY = q.syncEvery;
    sim.maxSub = q.maxSub;
    if (sim2) sim2.maxSub = q.maxSub;
    renderer.setPixelRatio(Math.min(devicePixelRatio, q.dpr));
    composer.setPixelRatio(Math.min(devicePixelRatio, q.dpr));
    if (r2) { r2.setPixelRatio(Math.min(devicePixelRatio, q.dpr)); sizePennant(); }
    resize();
  }
  // start one tier down on hardware that announces itself as modest
  if ((navigator.hardwareConcurrency || 8) <= 4 ||
      (navigator.deviceMemory || 8) <= 4) applyQuality(1);

  /* ---------- the closing pennant: the same cloth, hung short ----------
     A second BannerSim on its own small canvas: the identical verlet
     engine, wind field, jacquard-and-fleece material stack and cavity
     shading, authored as a short vertical drop from a gold rod with the
     swallowtail only at the foot. It steps and renders only while the
     close section is on screen. */
  const P_W = 1.4, PSX = 96, PSY = 30;
  /* the hero's sweep, mirrored: pinned beyond the top LEFT, the free
     swallowtail end reaching the lower RIGHT */
  const pCurve = new THREE.CatmullRomCurve3([
    new THREE.Vector3(3.8, -1.65, 0.06),   // free end: out through the RIGHT edge
    new THREE.Vector3(1.6, -0.9, -0.02),
    new THREE.Vector3(0.05, -0.06, 0.04),
    new THREE.Vector3(-0.62, 0.14, -0.04),
    new THREE.Vector3(-1.7, 0.55, 0.02),
    new THREE.Vector3(-3.7, 0.7, 0.0),     // pinned end: out through the LEFT edge
  ]);
  const P_LEN = pCurve.getLength();
  const _pt2 = new THREE.Vector3(), _tan2 = new THREE.Vector3();
  let sim2 = null, geo2 = null, mat2 = null, cav2 = null, r2 = null, sc2 = null, cam2 = null;
  let pTick = 0;

  function initPennant() {
    if (sim2) return;
    const cv = document.getElementById('closeBanner');
    if (!cv) return;
    try {
      r2 = new THREE.WebGLRenderer({ canvas: cv, antialias: true, alpha: true });
    } catch (e) { cv.remove(); return; }
    r2.setClearColor(0x000000, 0);
    r2.setPixelRatio(Math.min(devicePixelRatio, 1.5));
    r2.outputColorSpace = THREE.SRGBColorSpace;
    r2.toneMapping = THREE.NoToneMapping;
    sc2 = new THREE.Scene();
    cam2 = new THREE.OrthographicCamera(-0.7, 0.7, 2.0, -2.0, 0.1, 12);
    cam2.position.z = 6;
    const pm2 = new THREE.PMREMGenerator(r2);
    sc2.environment = pm2.fromScene(new RoomEnvironment(), 0.04).texture;
    pm2.dispose();
    const rimA2 = new THREE.DirectionalLight(0x6FA0FF, 0.85);
    rimA2.position.set(-4, 2.5, -3);
    const rimB2 = new THREE.DirectionalLight(0x3A7CFF, 0.65);
    rimB2.position.set(4.5, -1.5, -2.5);
    const key2 = new THREE.DirectionalLight(0xffffff, 0.7);
    key2.position.set(1.5, 3, 4);
    sc2.add(rimA2, rimB2, key2);

    // the hero's kind of sweep, laid left to right across the whole
    // screen: both ends run off the frame, so there is no cut to see
    sim2 = new BannerSim(P_LEN, P_W, PSX, PSY, (u, v) => {
      // the same swallowtail treatment as the hero, on the visible end
      if (u === 0 && (v === 0 || v === 1)) {
        u += 0.35 / PSX;
        v = v === 0 ? 0.35 / PSY : 1 - 0.35 / PSY;
      }
      const dAbs = Math.sqrt((v - 0.5) * (v - 0.5) + R_EPS * R_EPS) - R_EPS;
      const uMin = Math.max(0, 0.085 * (1 - 2 * dAbs));
      u = uMin + (1 - 2 * uMin) * u;
      pCurve.getPointAt(u, _pt2);
      pCurve.getTangentAt(u, _tan2);
      const nx = -_tan2.y, ny = _tan2.x;
      const off = (v - 0.5) * P_W;
      return [
        _pt2.x + nx * off,
        _pt2.y + ny * off,
        _pt2.z + 0.02 * Math.sin(u * 9.0) + 0.015 * Math.sin(v * 6.2),
      ];
    });
    geo2 = new THREE.PlaneGeometry(1, 1, PSX, PSY);
    geo2.setAttribute('color',
      new THREE.BufferAttribute(new Float32Array(sim2.count * 3).fill(1), 3));
    cav2 = new Float32Array(sim2.count);
    mat2 = new THREE.MeshPhysicalMaterial({
      color: new THREE.Color('#FF7D51'),
      metalness: 0,
      roughness: 1.0,
      normalScale: new THREE.Vector2(0.9, 0.9),
      sheen: 0.45,
      sheenRoughness: 0.5,
      sheenColor: new THREE.Color('#FFD0B8'),
      envMapIntensity: 0.78,
      vertexColors: true,
      side: THREE.DoubleSide,
    });
    sim2.windMul = 0.35;                 // a much gentler breeze than the hero's
    sim2.pinWave = 0.05;                 // and a top edge that floats, not a rod
    sim2.maxSub = QT[QUALITY.tier].maxSub;
    r2.setPixelRatio(Math.min(devicePixelRatio, QT[QUALITY.tier].dpr));
    const mesh2 = new THREE.Mesh(geo2, mat2);
    mesh2.frustumCulled = false;
    sc2.add(mesh2);
    composePennantMaps();
    sizePennant();
    addEventListener('resize', sizePennant);
    // the anchor depends on section heights, which settle late (fonts):
    // re-fit once the faces are in and once the document has fully loaded
    document.fonts.ready.then(() => sizePennant());
    addEventListener('load', () => sizePennant());
    for (let i = 0; i < 90; i++) sim2.substep();
    syncPennant();
  }

  function sizePennant() {
    if (!r2) return;
    const cv = r2.domElement;
    const w = cv.clientWidth || 320, h = cv.clientHeight || 460;
    r2.setSize(w, h, false);
    const a = (w / h) * 2.0;      // the view matches the frustum height
    cam2.left = -a; cam2.right = a;
    // the canvas must never hang below the page end (it would stretch the
    // scroll height and leave a void under the footer): anchor its bottom
    // to the footer's bottom and raise the camera by the same distance so
    // the cloth stays visually where it was
    const closeEl2 = document.getElementById('download');
    const footEl = document.querySelector('.foot');
    if (closeEl2 && footEl) {
      const secH = closeEl2.offsetHeight;
      const naturalTop = 0.5 * secH - 0.52 * h;
      const desiredTop = (footEl.offsetTop + footEl.offsetHeight) -
                         closeEl2.offsetTop - h;
      cv.style.top = desiredTop + 'px';
      cv.style.transform = 'translateX(-50%)';
      cam2.position.y = (naturalTop - desiredTop) * (4.0 / h);
    }
    cam2.updateProjectionMatrix();
  }

  /* the same weave, composed at the pennant's own aspect (no print) */
  function composePennantMaps() {
    if (readyCount < 7 || !mat2 || mat2.map) return;
    const PVW = Math.round(P_LEN * 1000), PVH = Math.round(P_W * 1000);
    const zones = (cx2, w, h, drawFleece, flatFill, drawField) => {
      drawFleece();
      const bu = Math.round(BORDER_WORLD / P_LEN * w), bv = Math.round(BORDER_WORLD / P_W * h);
      cx2.save(); cx2.beginPath();
      cx2.rect(bu, bv, w - 2 * bu, h - 2 * bv); cx2.clip();
      flatFill(); cx2.restore();
      const fu = Math.round((BORDER_WORLD + GAP_WORLD) / P_LEN * w);
      const fv = Math.round((BORDER_WORLD + GAP_WORLD) / P_W * h);
      cx2.save(); cx2.beginPath();
      cx2.rect(fu, fv, w - 2 * fu, h - 2 * fv); cx2.clip();
      drawField(); cx2.restore();
    };
    const build = (pw, ph, baseFill, fleeceImg2, fieldImg2, fleeceDraw, fieldDraw) => {
      const c2 = document.createElement('canvas');
      c2.width = pw; c2.height = ph;
      const cx2 = c2.getContext('2d');
      cx2.scale(pw / PVW, ph / PVH);
      cx2.fillStyle = baseFill; cx2.fillRect(0, 0, PVW, PVH);
      zones(cx2, PVW, PVH,
        () => fleeceDraw(cx2),
        () => { cx2.fillStyle = baseFill; cx2.fillRect(0, 0, PVW, PVH); },
        () => fieldDraw(cx2));
      return c2;
    };
    const cc = build(3200, 727, '#FF7D51', flcPatImg, patImg,
      (cx2) => {
        cx2.globalAlpha = 0.15; cx2.globalCompositeOperation = 'soft-light';
        tileAcross(cx2, flcPatImg, PVW, PVH, FLEECE_TILE, FLEECE_TILE);
        cx2.globalAlpha = 1; cx2.globalCompositeOperation = 'source-over';
      },
      (cx2) => {
        cx2.globalAlpha = 0.18; cx2.globalCompositeOperation = 'soft-light';
        cx2.filter = 'contrast(1.15)';
        tileAcross(cx2, patImg, PVW, PVH, TILE_W, TILE_H);
        cx2.filter = 'none';
        cx2.globalAlpha = 1; cx2.globalCompositeOperation = 'source-over';
      });
    const tex2 = new THREE.CanvasTexture(cc);
    tex2.colorSpace = THREE.SRGBColorSpace;
    tex2.anisotropy = 4;
    mat2.map = tex2;
    mat2.color.set('#FFFFFF');
    const ncc = build(2048, 466, 'rgb(128, 128, 255)', flcNorImg, norImg,
      (cx2) => tileAcross(cx2, flcNorImg, PVW, PVH, FLEECE_TILE, FLEECE_TILE),
      (cx2) => tileAcross(cx2, norImg, PVW, PVH, TILE_W, TILE_H));
    const ntex2 = new THREE.CanvasTexture(ncc);
    ntex2.colorSpace = THREE.NoColorSpace;
    ntex2.anisotropy = 4;
    mat2.normalMap = ntex2;
    const rcc = build(1400, 318, 'rgb(185, 185, 185)', flcRoughImg, roughImg,
      (cx2) => tileAcross(cx2, flcRoughImg, PVW, PVH, FLEECE_TILE, FLEECE_TILE),
      (cx2) => tileAcross(cx2, roughImg, PVW, PVH, TILE_W, TILE_H));
    const rtex2 = new THREE.CanvasTexture(rcc);
    rtex2.colorSpace = THREE.NoColorSpace;
    rtex2.anisotropy = 4;
    mat2.roughnessMap = rtex2;
    mat2.needsUpdate = true;
  }

  function syncPennant() {
    const pos2 = geo2.attributes.position;
    pos2.array.set(sim2.positions);
    pos2.needsUpdate = true;
    if (pTick++ % SYNC_EVERY === 0) {
      geo2.computeVertexNormals();
      sim2.computeCavity(geo2.attributes.normal.array, cav2, 6);
      const col2 = geo2.attributes.color;
      for (let i = 0; i < sim2.count; i++) {
        const shade = 1 - 0.17 * cav2[i];
        col2.array[i * 3] = shade;
        col2.array[i * 3 + 1] = shade;
        col2.array[i * 3 + 2] = shade;
      }
      col2.needsUpdate = true;
      geo2.attributes.normal.needsUpdate = true;
    }
  }

  let pVisible = false, pRaf = 0, pLast = 0, pSizedW = 0, pSizedH = 0;
  function pennantLoop(now) {
    pRaf = pVisible ? requestAnimationFrame(pennantLoop) : 0;
    if (now - pLast < 15) return;
    const pdt = Math.min((now - pLast) / 1000, 0.05);
    pLast = now;
    // a canvas measured while the page was hidden reports a bogus size, and
    // a document whose height has shifted since sizing leaves the anchor
    // stale; re-fit the moment the real layout differs from what we sized for
    const cw = r2.domElement.clientWidth, dh = document.body.scrollHeight;
    if ((cw && cw !== pSizedW) || dh !== pSizedH) {
      pSizedW = cw; sizePennant(); pSizedH = document.body.scrollHeight;
    }
    sim2.step(pdt);
    syncPennant();
    r2.render(sc2, cam2);
  }
  const closeHost = document.getElementById('download');
  if (closeHost) {
    if (reduced) {
      initPennant();
      if (sim2) {
        for (let i = 0; i < 240; i++) sim2.substep();
        syncPennant();
        r2.render(sc2, cam2);
      }
    } else {
      new IntersectionObserver((en) => {
        pVisible = en[0].isIntersecting;
        if (pVisible) {
          initPennant();
          if (sim2 && !pRaf) { pLast = performance.now(); pRaf = requestAnimationFrame(pennantLoop); }
        }
      }, { rootMargin: '140px' }).observe(closeHost);
    }
  }

  /* ---------- loop ---------- */
  for (let i = 0; i < 90; i++) sim.substep();   // settle before first paint
  syncCloth();

  /* ---------- the fall, baked ----------
     The fall is still pure physics: the same sim, pins released, gravity,
     drag and turbulence acting on every vertex. But it runs ONCE as an
     invisible shadow pass spread over the first moments after load, and
     every frame of it is recorded. Scroll then owns the playhead: the
     banner falls exactly as far as you have scrolled, and scrolling back
     rewinds the very same fall. Near the top the recording cross-fades
     into the live hanging sim, so rest keeps its living wind. */
  const FALL_SNAP = 4;                          // record every 4th substep (30fps)
  const FALL_FRAMES_TOTAL = 121;                // 4.0s of fall, enough to fully clear
  const FALL_HP_END = 0.85;                     // scroll fraction where the fall completes
  const fallFrames = [];
  const fallScratch = new Float32Array(sim.count * 3);
  const liveSave = { p: new Float32Array(sim.count * 3), prev: new Float32Array(sim.count * 3), t: 0 };
  const shadow = { p: null, prev: null, t: 0, sub: 0 };
  let fallDone = false;
  let fallMap = null;     // scroll fraction -> frame, even in traveled distance
  let fallMapNext = null; // freshly built map, adopted only when no snap can show
  let fallMapScale = 0;   // rootScale the map was built for (rebuilt on resize)

  /* Linear time would front-load the scrub (the fall accelerates), so the
     playhead is re-parameterized by how far the fabric actually traveled:
     each unit of scroll moves the cloth the same amount on screen. */
  function buildFallMap() {
    const n3 = sim.count * 3;
    const gone = -1.15 / Math.max(0.4, rootScale);  // every vertex safely below view
    const cum = new Float64Array(fallFrames.length);
    let cutoff = fallFrames.length - 1;
    for (let k = 1; k < fallFrames.length; k++) {
      const A = fallFrames[k - 1], B = fallFrames[k];
      let d = 0, maxY = -Infinity;
      for (let j = 0; j < n3; j += 3) {
        const dx = B[j] - A[j], dy = B[j + 1] - A[j + 1], dz = B[j + 2] - A[j + 2];
        d += Math.sqrt(dx * dx + dy * dy + dz * dz);
        if (B[j + 1] > maxY) maxY = B[j + 1];
      }
      cum[k] = cum[k - 1] + d;
      if (cutoff === fallFrames.length - 1 && maxY < gone) cutoff = k;
    }
    const total = cum[cutoff] || 1;
    const map = new Float32Array(129);
    let k = 0;
    for (let m = 0; m <= 128; m++) {
      const target = (m / 128) * total;
      while (k < cutoff - 1 && cum[k + 1] < target) k++;
      const span = (cum[k + 1] - cum[k]) || 1;
      map[m] = Math.min(cutoff, k + (target - cum[k]) / span);
    }
    fallMapNext = map;
    fallMapScale = rootScale;
  }

  function bakeChunk(needFrames) {
    if (fallDone) return;
    if (!shadow.p) {                            // shadow starts from the settled pose
      shadow.p = sim.positions.slice();
      shadow.prev = sim.prev.slice();
      shadow.t = sim.time;
      fallFrames.push(sim.positions.slice());   // frame 0: hanging at rest
    }
    // a few substeps per frame in the background; a burst if scroll has
    // already outrun the recording (only possible in the first second)
    let budget = Math.min(48, Math.max(6, (needFrames - fallFrames.length) * FALL_SNAP));
    liveSave.p.set(sim.positions); liveSave.prev.set(sim.prev); liveSave.t = sim.time;
    sim.positions.set(shadow.p); sim.prev.set(shadow.prev); sim.time = shadow.t;
    const rips = sim.ripples; sim.ripples = null;
    sim.released = true; sim.windBoost = 1.8;
    while (budget-- > 0 && fallFrames.length < FALL_FRAMES_TOTAL) {
      sim.substep();
      if (++shadow.sub % FALL_SNAP === 0) fallFrames.push(sim.positions.slice());
    }
    if (fallFrames.length >= FALL_FRAMES_TOTAL) { fallDone = true; buildFallMap(); }
    shadow.p.set(sim.positions); shadow.prev.set(sim.prev); shadow.t = sim.time;
    sim.positions.set(liveSave.p); sim.prev.set(liveSave.prev); sim.time = liveSave.t;
    sim.ripples = rips;
    sim.released = false; sim.windBoost = 0;
    if (fallDone) { shadow.p = null; shadow.prev = null; }
  }


  window.__axm = { renderer, sim, bokehPass, DOF, QUALITY, setQuality: applyQuality, get inner() { return innerRef; }, fallInfo: () => ({ frames: fallFrames.length, done: fallDone, mapped: !!fallMap }) , fallFrames };  // dev diagnostics
  const aboutEl = document.getElementById('about');
  const aboutText = aboutEl ? aboutEl.querySelector('.about-text') : null;
  const heroTextEl = document.querySelector('.cta-mask') || document.getElementById('hero');
  const stageScrim = document.querySelector('.stage-scrim');
  const opaqueEl = document.getElementById('practice');
  const stickyEl = document.querySelector('.about-sticky');
  let scrollHeroP = 0, scrollAboutP = 0, keyTurn = 0, keyCalm = 0, keyDrift = 0;
  const clock = new THREE.Clock();
  let frames = 0;
  let lastFrame = 0;
  let lastScrub = -1;
  const tipOff = new THREE.Vector3();   // key-tip offset under the reveal pose
  const focusOff = new THREE.Vector3(); // tip under the reveal-only pose, for focus
  const revealEuler = new THREE.Euler();
  let heroFrame = 0;
  function queueHeroFrame() {
    if (!heroFrame && !document.hidden) heroFrame = requestAnimationFrame(tick);
  }
  function wakeHero() {
    if (!heroFrame) {
      if (document.hidden) return;
      if (opaqueEl && opaqueEl.getBoundingClientRect().top <= 0) {
        canvas.style.visibility = 'hidden';
        return;
      }
      clock.getDelta(); // resuming must not advance the cloth by the paused time
      queueHeroFrame();
    }
  }
  function tick(now) {
    heroFrame = 0;
    // The fixed hero is completely covered past the opaque practice section.
    // Stop before cloth, geometry and layout work, and wake only on navigation.
    if (document.hidden || (opaqueEl && opaqueEl.getBoundingClientRect().top <= 0)) {
      if (!document.hidden) canvas.style.visibility = 'hidden';
      lastFrame = 0;
      return;
    }
    if (canvas.style.visibility) canvas.style.visibility = '';
    if (!reduced && now - lastFrame < 15) {   // ~60fps cap on 120Hz displays
      queueHeroFrame();
      return;
    }
    const frameGap = lastFrame ? now - lastFrame : 16.7;
    lastFrame = now;
    if (!reduced) {
      // the health check: a loaded machine shows up as widening frame gaps
      perfAvg += (Math.min(frameGap, 120) - perfAvg) * 0.06;
      if (perfAvg > 26 && QUALITY.tier < QT.length - 1 &&
          now - QUALITY.lastChange > 1500) {
        applyQuality(QUALITY.tier + 1);
        QUALITY.lastChange = now;
        perfAvg = 16.7;
      } else if (perfAvg < 17.4 && QUALITY.tier > 0 &&
                 now - QUALITY.lastChange > 15000) {
        applyQuality(QUALITY.tier - 1);
        QUALITY.lastChange = now;
        perfAvg = 16.7;
      }
    }
    const dt = clock.getDelta();
    applyPointer(now);

    // scroll choreography: the banner falls away along its own diagonal,
    // the key starts turning with the first pixel of scroll and eases to
    // rest; the stanza appears on arrival and departs near the end
    const sy = window.scrollY;
    const ih = window.innerHeight;
    scrollHeroP = Math.min(1, sy / ih);
    // the banner's fall is the baked physics recording, scrubbed by
    // scroll: down plays it forward, up rewinds it along the same path
    const hp = scrollHeroP;
    if (!reduced) {
      const q = Math.min(1, hp / FALL_HP_END);
      if (!fallDone) bakeChunk(hp > 0.001 ? Math.ceil(q * (FALL_FRAMES_TOTAL - 1)) + 1 : 0);
      else if (fallMapScale !== rootScale) buildFallMap();  // window was resized
      if (fallMapNext && (hp <= 0.001 || hp >= FALL_HP_END)) {
        fallMap = fallMapNext; fallMapNext = null;  // adopt only when no snap can show
      }
      const H0 = 0.05;                          // live-to-recording blend window
      if (hp < H0) sim.step(dt);                // the hanging banner keeps living
      else sim.time += Math.min(dt, 0.05);      // beak and bob keep their clock
      if (hp <= 0.001 || fallFrames.length < 2) {
        syncCloth();
        lastScrub = -1;
      } else {
        let tf;
        if (fallMap) {
          const mq = q * 128, mi = Math.min(127, mq | 0);
          tf = fallMap[mi] + (fallMap[mi + 1] - fallMap[mi]) * (mq - mi);
        } else {
          tf = q * (FALL_FRAMES_TOTAL - 1);   // bake still finishing (first second)
        }
        const k = Math.min(fallFrames.length - 2, tf | 0);
        const fr = Math.min(1, tf - k);
        const x = Math.min(1, hp / H0);
        const w = x * x * (3 - 2 * x);
        if (w < 1 || k + fr !== lastScrub) {    // skip geometry work when parked
          const A = fallFrames[k], B = fallFrames[k + 1];
          const S = fallScratch, live = sim.positions;
          if (w >= 1) {
            for (let j = 0; j < S.length; j++) S[j] = A[j] + (B[j] - A[j]) * fr;
          } else {
            for (let j = 0; j < S.length; j++) {
              const sc = A[j] + (B[j] - A[j]) * fr;
              S[j] = live[j] + (sc - live[j]) * w;
            }
          }
          sim.positions = S;
          if (w >= 1) syncTick &= ~1;   // the frame we park on gets fresh normals/AO
          syncCloth();
          sim.positions = live;
          lastScrub = k + fr;
        }
      }
    } else {
      // static captures still need the resolved end states
      const fall = Math.pow(hp, 1.75);
      cloth.position.set(-5.0 * hp, -3.9 * fall, 0);
      cloth.rotation.z = -0.30 * fall;
      sim.step(dt);
      syncCloth();
    }
    if (aboutEl) {
      const travel = Math.max(1, aboutEl.offsetHeight - ih);
      scrollAboutP = Math.min(1, Math.max(0, (sy - aboutEl.offsetTop) / travel));
      const gs = Math.min(1, sy / (aboutEl.offsetTop + travel * 0.45));
      const e = 1 - Math.pow(1 - gs, 3);      // spinning from the first scroll
      // lands with the bit's blade facing the camera (measured from the
      // mesh: blade normal is local +z at yaw 3.105; rest yaw is -0.45)
      // the turn never fully stops: once it has settled into the revealed
      // pose a very slow drift keeps the gold alive through the hold, under
      // the demo window, and right up to the moment the key leaves the
      // screen. It accumulates only from the settle, so the landing angle
      // (bit square to the camera) is the same however long the visit took.
      // the slow drift only ever belongs to the revealed pose: it is scaled
      // by the reveal and unwinds once the key leaves it, so back at the
      // top the bird always holds its load pose exactly
      if (e > 0.99) keyDrift += dt * 0.05;
      else keyDrift *= Math.pow(0.05, dt);
      keyTurn = e * (9.84 + keyDrift);
      keyCalm = e;
      // nothing exits: once the turn settles, the key and the stanza simply
      // hold their pose until the demo section covers them over
      if (aboutText) {
        // the stanza arrives the moment the hero text block is out of view
        const heroGone = !heroTextEl || heroTextEl.getBoundingClientRect().bottom < 10;
        const stanzaIn = heroGone && sy > 10;
        aboutText.classList.toggle('in', stanzaIn);
        if (stageScrim) {
          stageScrim.classList.toggle('in', stanzaIn && scrollAboutP < 0.8);
        }
      }
    }
    // the fallen cloth parks far below the stage; once its scrub is spent
    // it must never be seen again - the camera lift below would otherwise
    // drag its resting place back into view around the practice section
    cloth.visible = hp < FALL_HP_END;
    // the scene is painted on a FIXED canvas, so nothing in it scrolls on its
    // own: past the moment the stage lets go, lift the camera by exactly the
    // distance scrolled, and the key travels up with the page instead of
    // sitting still and reappearing in the gap under the demo window
    if (aboutEl && stickyEl) {
      const release = aboutEl.offsetTop + aboutEl.offsetHeight - stickyEl.offsetHeight;
      const past = Math.max(0, sy - release);
      camera.position.y = -(past / ih) * 2;    // ortho view is 2 world units tall
    }
    const t = sim.time;
    if (innerRef) {
      // beak idle fades as the key performs its slow opening turn
      const idle = 0.175 * (0.5 + 0.5 * Math.sin(t * 0.45)) * (1 - 0.7 * keyCalm);
      innerRef.rotation.y = MODEL.ry - idle + keyTurn;
    }
    const ks = 1 + 0.35 * keyCalm;   // the key comes closer, growing as it turns
    modelGroup.scale.setScalar(ks);
    modelGroup.rotation.x = -0.35 * keyCalm;  // tip toward the viewer, bird away
    modelGroup.rotation.z = -0.61 * keyCalm;  // the reveal pose holds to the end
    // the reveal anchors the TIP: it settles just left of and below the
    // stanza's center, and the entry drifts it to dead center
    tipOff.set(0, -1.74, 0).applyEuler(modelGroup.rotation).multiplyScalar(ks);
    const heroY = MODEL.y + 0.018 * Math.sin(t * 0.5) * (1 - 0.6 * keyCalm);
    const goalX = (-0.10 * camera.right) / rootScale - tipOff.x;
    const goalY = (-0.10 - root.position.y) / rootScale - tipOff.y;
    modelGroup.position.x = MODEL.x + (goalX - MODEL.x) * keyCalm;
    modelGroup.position.y = heroY + (goalY - heroY) * keyCalm;
    modelGroup.position.z = MODEL.z;
    // the focus plane belongs to the REVEAL pose; the entering key leaves it
    revealEuler.set(-0.35 * keyCalm, 0, -0.61 * keyCalm);
    focusOff.set(0, -1.74, 0).applyEuler(revealEuler);
    const tipZunit = focusOff.z;

    if (!reduced && keyCalm > DOF.on && QT[QUALITY.tier].dof) {
      bokehPass.uniforms.aperture.value =
        DOF.aperture * Math.min(1, (keyCalm - DOF.on) / DOF.ramp);
      // focus rides the key's tip: its depth depends on the tilt and the
      // responsive scene scale, so the sharp plane is computed, not fixed
      bokehPass.uniforms.focus.value =
        6 - (MODEL.z + tipZunit * 0.92 * ks) * rootScale;
      composer.render();
    } else {
      renderer.render(scene, camera);
    }
    if (reduced && frames++ > 0) return;
    queueHeroFrame();
  }
  if (reduced) { for (let i = 0; i < 360; i++) sim.substep(); }
  addEventListener('scroll', wakeHero, { passive: true });
  addEventListener('resize', wakeHero);
  document.addEventListener('visibilitychange', wakeHero);
  queueHeroFrame();
}

/* ?clean hides the text block for design screenshots; ?clean=all hides the logo too */
(function () {
  const m = location.search.match(/clean(=all)?/);
  if (m) {
    document.getElementById('hero').style.display = 'none';
    if (m[1]) document.querySelector('.nav').style.display = 'none';
  }
})();

/* scroll reveals for the sections below the stage */
(function () {
  const els = document.querySelectorAll('.rv');
  if (reduced || !('IntersectionObserver' in window)) {
    els.forEach((el) => el.classList.add('in'));
    return;
  }
  const io = new IntersectionObserver((ents) => {
    for (const en of ents) {
      if (en.isIntersecting) { en.target.classList.add('in'); io.unobserve(en.target); }
    }
  }, { threshold: 0.2, rootMargin: '0px 0px -8% 0px' });
  els.forEach((el) => io.observe(el));
})();

/* the demo is pulled up over the locked stage by exactly the amount that
   puts its window at the centre of the screen at the moment the stage lets
   go, so the key and the stanza hold still until the demo is centred and
   everything then travels up together */
function layoutDemoOverlap() {
  const demo = document.getElementById('demo');
  const win = document.querySelector('.demo-window');
  const sticky = document.querySelector('.about-sticky');
  if (!demo || !win || !sticky) return;
  const padTop = parseFloat(getComputedStyle(demo).paddingTop) || 0;
  const winH = win.getBoundingClientRect().height;
  // the stage lets go when its sticky frame reaches the section's end; the
  // demo is pulled up by just enough that its window is centred right then
  const stickyH = sticky.getBoundingClientRect().height;
  demo.style.marginTop = -(stickyH - innerHeight / 2 + padTop + winH / 2) + 'px';
}
addEventListener('resize', layoutDemoOverlap);
layoutDemoOverlap();

/* The demo rises over the locked stage faster than the page scrolls: it is
   held down by an extra offset that eases to nothing, so the window covers
   the whole distance from below the fold to screen centre in about half a
   screen of scrolling instead of a full one. At the end of that ramp the
   offset is zero, which is exactly where the stage lets go. */
const DEMO_COVER = 0.45;              // fraction of a screen the cover takes
(function () {
  const demo = document.getElementById('demo');
  const about = document.getElementById('about');
  const sticky = document.querySelector('.about-sticky');
  const win = document.querySelector('.demo-window');
  if (!demo || !about || !sticky || !win) return;
  let queued = 0;
  function placeDemo() {
    queued = 0;
    const ih = innerHeight;
    const cover = ih * DEMO_COVER;
    const release = about.offsetTop + about.offsetHeight - sticky.offsetHeight;
    const lead = Math.max(0, ih / 2 + win.getBoundingClientRect().height / 2 - cover);
    const p = Math.min(1, Math.max(0, (scrollY - (release - cover)) / cover));
    const e = p * p * (3 - 2 * p);
    demo.style.transform = 'translate3d(0,' + ((1 - e) * lead).toFixed(1) + 'px,0)';
  }
  const request = () => { if (!queued) queued = requestAnimationFrame(placeDemo); };
  addEventListener('scroll', request, { passive: true });
  addEventListener('resize', request);
  window.__placeDemo = placeDemo;     // re-run once the window has its height
  placeDemo();
})();

/* the demo window sizes itself to the app document inside it.
   body.scrollHeight is the CONTENT height; documentElement's would be
   clamped up to the iframe viewport and could never shrink the frame */
(function () {
  const frame = document.getElementById('demoFrame');
  if (!frame) return;
  const fit = () => {
    const d = frame.contentDocument;
    if (!d || !d.body) return;
    const h = d.body.scrollHeight;
    if (h > 120) frame.style.height = h + 'px';
    layoutDemoOverlap();          // the overlap follows the window's height
    if (window.__placeDemo) window.__placeDemo();
  };
  const arm = () => {
    fit();
    try { new ResizeObserver(fit).observe(frame.contentDocument.body); } catch (e) {}
    setTimeout(fit, 600);               // again once fonts inside settle
  };
  frame.addEventListener('load', arm);
  const d0 = frame.contentDocument;     // already loaded before we attached?
  if (d0 && d0.readyState === 'complete' && d0.body && d0.body.childElementCount) arm();
  addEventListener('resize', fit);
})();

/* headline reveal: fire once fonts are ready so lines never reflow mid-rise */
document.fonts.ready.then(() => {
  requestAnimationFrame(() => document.documentElement.classList.add('hero-in'));
});

/* verification hooks */
(function () {
  const m = location.search.match(/go=([a-z0-9-]+)(?:&off=(-?\d+))?/);
  if (m) {
    const el = document.getElementById(m[1]);
    if (el) document.body.style.marginTop = (-(el.offsetTop + (m[2] ? parseInt(m[2], 10) : -60))) + 'px';
  }
})();
