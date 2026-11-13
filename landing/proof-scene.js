import * as THREE from 'three';
import { EffectComposer } from 'three/addons/postprocessing/EffectComposer.js';
import { RenderPass } from 'three/addons/postprocessing/RenderPass.js';
import { BokehPass } from 'three/addons/postprocessing/BokehPass.js';
import { OutputPass } from 'three/addons/postprocessing/OutputPass.js';
import { createProofCards } from './proof-cards.js';
import { optimizeProofDepthOfField } from './proof-dof.js';
import { createProofFraming, createFlight, sampleFlight, sampleProofFocus, sampleProofScroll, FLIGHT_OFFSETS } from './proof-flight.js';

const chapter = document.querySelector('#proof');
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const clamp = THREE.MathUtils.clamp;
const smooth = (a, b, value) => THREE.MathUtils.smootherstep(value, a, b);

// No wheel handler: the page already smooths wheel scrolling. Touch, keyboard,
// anchor navigation and reversed scrolling all drive this same native timeline.
if (chapter && !reducedMotion.matches && !location.search.includes('static')) {
  let loading = false;
  const preload = new IntersectionObserver((entries) => {
    if (!entries.some((entry) => entry.isIntersecting) || loading) return;
    loading = true;
    preload.disconnect();
    mountProofScene().catch((error) => {
      chapter.classList.remove('proof-ready');
      chapter.querySelectorAll('.card').forEach((card) => card.classList.add('in'));
      console.warn('The proof chapter is using its readable card fallback.', error);
    });
  }, { rootMargin: '1600px' });
  preload.observe(chapter);
}

function studioEnvironment(renderer) {
  const room = new THREE.Scene();
  room.background = new THREE.Color(0x303038);
  const boxes = [
    { p: [-4, 5, 5], size: [4.5, 8], color: 0xfff2d6, strength: 5 },
    { p: [5, 1, 3], size: [1.3, 7], color: 0xffffff, strength: 7 },
    { p: [0, 7, -1], size: [6, 3], color: 0xffffff, strength: 3 },
    { p: [-1, .6, 8], size: [5, 7], color: 0xfff7e5, strength: 1.8 },
    { p: [-1, -4, 5], size: [5, 2], color: 0xcdd8ef, strength: .8 },
  ];
  for (const box of boxes) {
    const panel = new THREE.Mesh(
      new THREE.PlaneGeometry(...box.size),
      new THREE.MeshBasicMaterial({ color: new THREE.Color(box.color).multiplyScalar(box.strength), side: THREE.DoubleSide }),
    );
    panel.position.set(...box.p);
    panel.lookAt(0, 0, 0);
    room.add(panel);
  }
  const generator = new THREE.PMREMGenerator(renderer);
  const target = generator.fromScene(room, .025, .1, 40);
  generator.dispose();
  room.traverse((object) => { object.geometry?.dispose(); object.material?.dispose(); });
  return target;
}

async function mountProofScene() {
  const stage = chapter.querySelector('.proof-stage');
  const track = chapter.querySelector('.proof-scroll');
  const canvas = chapter.querySelector('.proof-canvas');
  const articles = [...chapter.querySelectorAll('.card')];
  const colors = ['#c56a51', '#456a93', '#cbb891', '#6f8e75'];
  const data = articles.map((article, index) => ({
    numeral: article.querySelector('.card-head span:first-child').textContent,
    name: article.querySelector('.card-head span:last-child').textContent,
    copy: article.querySelector('p').textContent,
    art: article.querySelector('img').src,
    color: colors[index],
  }));
  let renderer;
  try {
    renderer = new THREE.WebGLRenderer({ canvas, alpha: true, premultipliedAlpha: false, antialias: false, powerPreference: 'high-performance' });
  } catch (error) { throw error; }
  const profile = ['127.0.0.1', 'localhost'].includes(location.hostname)
    && new URLSearchParams(location.search).has('profile')
    ? await import(new URL('./scripts/proof-profile.js', document.baseURI).href).then(module => module.createProofProfile(renderer)).catch(() => null) : null;
  renderer.setClearColor(0x000000, 0);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.04;
  renderer.shadowMap.enabled = true;
  // PCF accepts a wider filter radius; r160's PCFSoft variant fixes its radius.
  renderer.shadowMap.type = THREE.PCFShadowMap;
  // Update once per changed pose, not again for the depth-of-field depth pass.
  renderer.shadowMap.autoUpdate = false;

  const scene = new THREE.Scene();
  scene.fog = new THREE.Fog(0xf2f4fa, 17, 31);
  const camera = new THREE.PerspectiveCamera(36, 1, .1, 70);
  const environment = studioEnvironment(renderer);
  scene.environment = environment.texture;

  const key = new THREE.DirectionalLight(0xffedcf, 1.8);
  key.position.set(-3.5, 5, 6);
  key.castShadow = true;
  key.shadow.mapSize.set(1024, 1024);
  key.shadow.radius = 8;
  Object.assign(key.shadow.camera, { left: -9, right: 9, top: 6, bottom: -6, near: .1, far: 24 });
  key.shadow.bias = -.00015;
  key.shadow.normalBias = .004;
  scene.add(key);
  const fill = new THREE.DirectionalLight(0xe1eaff, .8);
  fill.position.set(4, 1, 5);
  scene.add(fill, new THREE.HemisphereLight(0xfffaf0, 0x8e96a6, .65));

  let cards;
  try {
    cards = await createProofCards(THREE, renderer, data);
  } catch (error) {
    environment.dispose();
    renderer.dispose();
    throw error;
  }
  // Each card presents at the foreground reading plane before joining its
  // permanent slot. All final faces share z=0 for the closing focus pull.
  cards.forEach((card) => {
    card.traverse((mesh) => {
      if (!mesh.isMesh) return;
      // The paper casts the same card silhouette without re-drawing all of the
      // tiny raised glyphs into shadow/depth maps on every scroll frame.
      mesh.castShadow = mesh.name === 'Cotton paper edge';
      if (mesh.castShadow) mesh.layers.enable(1);
    });
    scene.add(card);
  });
  // Prepare programs and upload paper maps while the chapter is approaching,
  // distributing uploads so they do not all stall the first visible frame.
  renderer.compile(scene, camera);
  const warmTextures = new Set();
  cards.forEach(card => card.traverse(mesh => {
    if (!mesh.material) return;
    Object.values(mesh.material).forEach(value => { if (value?.isTexture) warmTextures.add(value); });
  }));
  for (const texture of warmTextures) {
    renderer.initTexture(texture);
    await new Promise(resolve => setTimeout(resolve, 0));
  }
  const flightSamples = cards.map(() => ({ visible: false, pose: new Float64Array(7) }));
  const focusSample = { visible: false, pose: new Float64Array(7) };
  let flights = [];

  const renderTarget = new THREE.WebGLRenderTarget(1, 1, {
    type: THREE.HalfFloatType,
    samples: renderer.capabilities.isWebGL2 ? 4 : 0,
  });
  const composer = new EffectComposer(renderer, renderTarget);
  const renderPass = new RenderPass(scene, camera);
  const bokeh = new BokehPass(scene, camera, { focus: 10, aperture: .011, maxblur: .028 });
  // r160's BokehPass forces alpha to 1. Keep the page background transparent,
  // unpremultiply the filtered colour before the final output transform, and
  // leave a narrow sharp band so the entire approaching card stays readable.
  bokeh.materialBokeh.fragmentShader = bokeh.materialBokeh.fragmentShader
    .replace('float factor = ( focus + viewZ );', 'float factor = sign(focus + viewZ) * max(abs(focus + viewZ) - 0.4, 0.0);')
    .replace('gl_FragColor.a = 1.0;', 'gl_FragColor.rgb /= max(gl_FragColor.a, 0.0001);')
    .replaceAll('texture2D( tColor, ', 'sampleBokeh( ')
    .replace('void main() {', `
      float centerViewZ;
      vec4 centerColor;
      vec4 sampleBokeh(vec2 uv) {
        float sampleZ = getViewZ(getDepth(uv));
        float sharp = 1.0 - smoothstep(0.4, 0.8, abs(focus + sampleZ));
        float inFront = smoothstep(0.4, 0.9, sampleZ - centerViewZ);
        return mix(texture2D(tColor, uv), centerColor, sharp * inFront);
      }
      void main() {`)
    .replace('float viewZ = getViewZ( getDepth( vUv ) );', `
      float viewZ = getViewZ(getDepth(vUv));
      centerViewZ = viewZ;
      centerColor = texture2D(tColor, vUv);`);
  // Depth-aware taps keep a sharp foreground edge from bleeding into the
  // blurred background, avoiding a coloured halo around the reading card.
  optimizeProofDepthOfField(bokeh);
  const output = new OutputPass();
  composer.addPass(renderPass);
  composer.addPass(bokeh);
  composer.addPass(output);

  // A diffuse pool of shadow gives the floating stock a scale and a height.
  const shadowCanvas = document.createElement('canvas');
  shadowCanvas.width = shadowCanvas.height = 128;
  const shadowContext = shadowCanvas.getContext('2d');
  const wash = shadowContext.createRadialGradient(64, 64, 0, 64, 64, 64);
  wash.addColorStop(0, 'rgba(67,56,45,0.16)');
  wash.addColorStop(.35, 'rgba(67,56,45,0.085)');
  wash.addColorStop(1, 'rgba(67,56,45,0)');
  shadowContext.fillStyle = wash;
  shadowContext.fillRect(0, 0, 128, 128);
  const shadowTexture = new THREE.CanvasTexture(shadowCanvas);
  const shadow = new THREE.Mesh(new THREE.PlaneGeometry(6.4, 1.1), new THREE.MeshBasicMaterial({
    map: shadowTexture, transparent: true, depthWrite: false, toneMapped: false,
  }));
  shadow.position.set(0, -2.67, -.5);
  scene.add(shadow);

  const state = { progress: 0, disposed: false, frame: 0, last: 0 };
  let viewportHeight = 1;
  let cardScale = 1;
  let runLength = 1;
  let flightLength = 1;
  let framing;
  const scrollSample = { position: 0, cameraY: 0 };
  let trackTop = 0;
  let entranceLead = 0;
  let inView = false;
  let renderWidth = 0, renderHeight = 0, renderRatio = 0;
  let qualityScale = 1, qualityFrames = 0, qualityTime = 0;

  function sizeRenderTargets(width, height) {
    const base = Math.min(devicePixelRatio, 1.5, Math.sqrt(2400000 / (width * height)));
    const pixelRatio = Math.max(Math.min(1, base), base * qualityScale);
    if (width === renderWidth && height === renderHeight && pixelRatio === renderRatio) return;
    renderer.setPixelRatio(pixelRatio);
    renderer.setSize(width, height, false);
    composer.setPixelRatio(pixelRatio);
    composer.setSize(width, height);
    renderWidth = width; renderHeight = height; renderRatio = pixelRatio;
  }
  const events = new AbortController();
  const opts = { passive: true, signal: events.signal };

  function measure() {
    const stageHeight = stage.clientHeight;
    viewportHeight = canvas.clientHeight;
    const width = canvas.clientWidth;
    // Frame only the area beneath the pinned title, leaving the heading clear.
    framing = createProofFraming(width, viewportHeight);
    cardScale = framing.cardScale;
    shadow.scale.set(cardScale, cardScale, 1);
    camera.aspect = framing.aspect;
    camera.position.z = framing.cameraZ;
    camera.position.y = 0;
    camera.updateProjectionMatrix();
    flights = cards.map((_, index) => createFlight(index, framing));
    // Bound GPU work on large Retina displays; preserve more than one sample
    // per CSS pixel on ordinary laptop screens without rendering 4K blur maps.
    sizeRenderTargets(width, viewportHeight);
    bokeh.uniforms.focus.value = camera.position.z;
    bokeh.uniforms.aperture.value = width < 700 ? .014 : .011;
    bokeh.uniforms.maxblur.value = width < 700 ? .04 : .028;
    trackTop = track.getBoundingClientRect().top + scrollY;
    entranceLead = stageHeight * .6;
    // Phones add real native scroll distance after the unchanged flight pace.
    // Only the camera moves through the resulting fixed vertical column.
    track.style.setProperty('--proof-stack-scroll', `${framing.stackScroll}px`);
    runLength = Math.max(1, track.offsetHeight - stageHeight + entranceLead);
    flightLength = Math.max(1, runLength - framing.stackScroll);
    wake();
  }

  function targetProgress() { return clamp((scrollY - trackTop + entranceLead) / runLength, 0, 1); }

  function draw(time) {
    state.frame = 0;
    if (state.disposed || !inView || document.hidden) return;
    const dt = Math.min((time - (state.last || time)) / 1000, .05);
    state.last = time;
    const damping = 1 - Math.exp(-dt * 24);
    const target = targetProgress();
    state.progress += (target - state.progress) * damping;
    const settled = Math.abs(target - state.progress) < 0.00001;
    if (settled) state.progress = target;
    // On a busy or slower Retina device, trade excess render pixels for smooth
    // scrolling. Two measured reductions at most; never below one CSS pixel.
    // Keep the chosen budget while reading to avoid repeated reallocations.
    if (!settled && qualityScale > .73) {
      qualityTime += dt * 1000;
      if (++qualityFrames >= 45) {
        if (qualityTime / qualityFrames > 24) {
          qualityScale = Math.max(.72, qualityScale * .85);
          sizeRenderTargets(renderWidth, viewportHeight);
        }
        qualityTime = qualityFrames = 0;
      }
    }
    const p = state.progress;
    const { position, cameraY } = sampleProofScroll(framing, p * runLength, flightLength, scrollSample);
    camera.position.y = cameraY;
    bokeh.uniforms.focus.value = camera.position.z - sampleProofFocus(flights, position, focusSample);
    const presence = smooth(0, .048, Math.min(1, p * runLength / flightLength));
    stage.style.setProperty('--proof-presence', presence.toFixed(4));
    let shadowWeight = 0, shadowX = 0, shadowY = 0;
    const halfFov = Math.tan(camera.fov * Math.PI / 360);
    cards.forEach((card, index) => {
      const sample = sampleFlight(flights[index], position - FLIGHT_OFFSETS[index], flightSamples[index]);
      card.visible = sample.visible;
      if (!sample.visible) return;
      const [x, y, z, pitch, yaw, roll, scale] = sample.pose;
      // Project authored screen-space sweeps into actual 3D positions. The
      // alternating entry directions work on both phones and wide screens.
      const halfHeightAtDepth = (camera.position.z - z) * halfFov;
      card.position.set(x * halfHeightAtDepth * camera.aspect, y * halfHeightAtDepth, z);
      card.rotation.set(pitch, yaw, roll);
      card.scale.setScalar(scale);
      const localTime = position - FLIGHT_OFFSETS[index];
      const weight = smooth(-.35, -.05, localTime) * (1 - smooth(.4, .9, localTime));
      shadowX += card.position.x * weight;
      shadowY += card.position.y * weight;
      shadowWeight += weight;
    });
    // The faint shadow travels with the near paper instead of anchoring a stage.
    if (shadowWeight > .001) shadow.position.set(shadowX / shadowWeight, shadowY / shadowWeight - 2.62 * cardScale, -.5);
    shadow.material.opacity = Math.min(1, shadowWeight) * .42;
    key.position.x = -3.5 + Math.sin(position * 1.7) * .45;
    renderer.shadowMap.needsUpdate = true;
    profile?.before();
    composer.render();
    profile?.after(time);
    // Lighting and motion are scroll-driven, so an unchanged image costs no
    // frames. Scroll, layout and visibility events wake it immediately.
    if (!settled) state.frame = requestAnimationFrame(draw);
  }

  function wake() {
    if (state.disposed || state.frame || !inView || document.hidden) return;
    state.last = performance.now() - 1000 / 60;
    state.frame = requestAnimationFrame(draw);
  }
  window.addEventListener('resize', measure, opts);
  window.addEventListener('scroll', wake, opts);
  document.addEventListener('visibilitychange', wake, opts);

  function dispose() {
    if (state.disposed) return;
    state.disposed = true;
    cancelAnimationFrame(state.frame);
    events.abort();
    viewObserver.disconnect();
    layoutObserver.disconnect();
    chapter.classList.remove('proof-ready');
    const geometries = new Set();
    const materials = new Set();
    const textures = new Set();
    scene.traverse((object) => {
      if (object.geometry) geometries.add(object.geometry);
      for (const material of [object.material].flat().filter(Boolean)) {
        materials.add(material);
        for (const value of Object.values(material)) if (value?.isTexture) textures.add(value);
      }
    });
    geometries.forEach((geometry) => geometry.dispose());
    materials.forEach((material) => material.dispose());
    textures.forEach((texture) => texture.dispose());
    key.shadow.map?.dispose();
    bokeh.dispose();
    output.dispose();
    renderPass.dispose();
    composer.dispose();
    environment.dispose();
    renderer.dispose();
    profile?.dispose();
    articles.forEach((article) => article.classList.add('in'));
  }

  const viewObserver = new IntersectionObserver(([entry]) => {
    inView = entry.isIntersecting;
    if (!inView) {
      cancelAnimationFrame(state.frame);
      state.frame = 0;
    }
    if (inView) {
      state.progress = targetProgress();
      wake();
    }
  }, { rootMargin: '150px' });
  const layoutObserver = new ResizeObserver(measure);
  if (reducedMotion.matches) { dispose(); return; }
  // Enhancement becomes visible only after all four cards are complete.
  profile?.ready();
  chapter.classList.add('proof-ready');
  articles.forEach((article) => article.classList.add('in'));
  measure();
  state.progress = targetProgress();
  viewObserver.observe(stage);
  layoutObserver.observe(document.body);
  layoutObserver.observe(stage);
  const demo = document.querySelector('.demo-window');
  if (demo) layoutObserver.observe(demo);
  reducedMotion.addEventListener('change', (event) => { if (event.matches) dispose(); }, { signal: events.signal });
  canvas.addEventListener('webglcontextlost', (event) => { event.preventDefault(); dispose(); }, { signal: events.signal });
  window.addEventListener('pagehide', (event) => { if (!event.persisted) dispose(); }, { signal: events.signal });
}
