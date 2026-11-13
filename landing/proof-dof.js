import * as THREE from 'three';
import { FullScreenQuad } from 'three/addons/postprocessing/Pass.js';

// Keep the reading card at full resolution. Only the already-defocused image
// needs the expensive 41-tap filter, which runs at half width and height.
export function optimizeProofDepthOfField(bokeh) {
  // RGBADepthPacking encodes four bytes. Half-float framebuffer conversion
  // can truncate those channels enough to defocus the farther mobile camera.
  // Keep depth byte-exact; only the HDR colour buffers need half floats.
  bokeh.renderTargetDepth.texture.type = THREE.UnsignedByteType;
  const blurred = new THREE.WebGLRenderTarget(1, 1, {
    type: THREE.HalfFloatType, depthBuffer: false,
  });
  const composite = new THREE.ShaderMaterial({
    depthTest: false, depthWrite: false,
    uniforms: {
      sharp: { value: null }, blurred: { value: blurred.texture },
      depth: { value: bokeh.renderTargetDepth.texture },
      focus: bokeh.uniforms.focus,
      nearClip: bokeh.uniforms.nearClip, farClip: bokeh.uniforms.farClip,
    },
    vertexShader: `varying vec2 vUv;
      void main() { vUv = uv; gl_Position = vec4(position.xy, 0.0, 1.0); }`,
    fragmentShader: `
      #include <packing>
      varying vec2 vUv;
      uniform sampler2D sharp, blurred, depth;
      uniform float focus, nearClip, farClip;
      void main() {
        float z = perspectiveDepthToViewZ(unpackRGBAToDepth(texture2D(depth, vUv)), nearClip, farClip);
        float defocus = smoothstep(0.4, 0.7, abs(focus + z));
        vec4 crisp = texture2D(sharp, vUv);
        gl_FragColor = mix(crisp, texture2D(blurred, vUv), defocus);
        gl_FragColor.rgb /= max(gl_FragColor.a, 0.0001);
      }`,
  });
  const quad = new FullScreenQuad(composite);
  // A focused pixel has a zero-radius kernel: one sample is exactly equivalent.
  // Filter and upsample premultiplied colour, then unpremultiply once above.
  // This prevents transparent black from darkening blurred card/shadow edges.
  bokeh.materialBokeh.fragmentShader = bokeh.materialBokeh.fragmentShader
    .replaceAll('gl_FragColor.rgb /= max(gl_FragColor.a, 0.0001);', '')
    .replace(
    'vec2 dofblur =',
    `if (abs(factor) < 0.0001) {
      gl_FragColor = centerColor;
      return;
    }
    vec2 dofblur =`,
  );
  const render = bokeh.render.bind(bokeh);
  const resize = bokeh.setSize.bind(bokeh);
  const dispose = bokeh.dispose.bind(bokeh);
  bokeh.setSize = (width, height) => {
    resize(width, height);
    blurred.setSize(Math.ceil(width / 2), Math.ceil(height / 2));
  };
  bokeh.render = (renderer, writeBuffer, readBuffer) => {
    const layers = bokeh.camera.layers.mask;
    const toScreen = bokeh.renderToScreen;
    // The solid paper silhouette supplies depth. Subpixel lettering and foil
    // have the same focus, and the transparent shadow wash must not write depth.
    bokeh.camera.layers.set(1);
    bokeh.renderToScreen = false;
    try { render(renderer, blurred, readBuffer); }
    finally {
      bokeh.camera.layers.mask = layers;
      bokeh.renderToScreen = toScreen;
    }
    composite.uniforms.sharp.value = readBuffer.texture;
    renderer.setRenderTarget(toScreen ? null : writeBuffer);
    quad.render(renderer);
  };
  bokeh.dispose = () => {
    dispose(); blurred.dispose(); composite.dispose(); quad.dispose();
  };
}
