import { SVGLoader } from 'three/addons/loaders/SVGLoader.js';
import { mergeGeometries, mergeVertices } from 'three/addons/utils/BufferGeometryUtils.js';

// Outlines are a small subset of the same local PP Cirka font used on the page.
// Canvas supplies the browser's kerning and line breaks; the letters themselves
// are bevelled solids, so the studio lights illuminate their faces and edges.
export async function createLetteringFactory(THREE) {
  const response = await fetch(new URL('./fonts/PPCirka-card-outlines.json', document.baseURI));
  if (!response.ok) throw new Error('Could not load the raised card lettering.');
  const font = await response.json();
  const loader = new SVGLoader();
  const ctx = document.createElement('canvas').getContext('2d');
  const cache = new Map();
  const materials = new Map();
  const W = 2.8, H = 4.4, TW = 1536, TH = Math.round(TW * H / W);
  const pixel = W / TW, face = .045 / 2;

  function glyphGeometry(char, size) {
    const key = `${size}:${char}`;
    if (cache.has(key)) return cache.get(key);
    if (!(char in font.glyphs)) throw new Error(`Missing card lettering glyph: ${char}`);
    const outline = font.glyphs[char];
    if (!outline) return null;
    const parsed = loader.parse(`<svg xmlns="http://www.w3.org/2000/svg"><path d="${outline}"/></svg>`);
    const shapes = parsed.paths.flatMap(path => SVGLoader.createShapes(path));
    const scale = size / font.unitsPerEm * pixel;
    const geometry = new THREE.ExtrudeGeometry(shapes, {
      depth: (size >= 140 ? .007 : .005) / scale,
      steps: 1, curveSegments: 4,
      bevelEnabled: true, bevelSegments: 2,
      bevelThickness: .0012 / scale,
      bevelSize: (size >= 140 ? .00075 : .0005) / scale,
    });
    geometry.scale(scale, scale, scale);
    // This solid metal uses no texture maps. Keep its bevels and curves while
    // sharing repeated triangle vertices before the glyph is copied into lines.
    geometry.deleteAttribute('uv');
    const indexed = mergeVertices(geometry, 1e-7);
    geometry.dispose();
    cache.set(key, indexed);
    return indexed;
  }

  function line(parts, text, size, baseline) {
    ctx.font = `400 ${size}px "PP Cirka", serif`;
    const left = (TW - ctx.measureText(text).width) / 2;
    [...text].forEach((char, i) => {
      const glyph = glyphGeometry(char, size);
      if (!glyph) return;
      const x = left + ctx.measureText(text.slice(0, i + 1)).width - ctx.measureText(char).width;
      const geometry = glyph.clone();
      geometry.translate((x - TW / 2) * pixel, H / 2 - baseline * pixel, face + .0018);
      parts.push(geometry);
    });
  }

  return {
    create(item) {
      const parts = [];
      line(parts, item.numeral, 51, TH * .075);
      line(parts, item.name, 145, TH * .17);
      ctx.font = '400 89px "PP Cirka", serif';
      const lines = [];
      let current = '';
      for (const word of item.copy.split(/\s+/)) {
        const next = current ? `${current} ${word}` : word;
        if (current && ctx.measureText(next).width > TW * .79) {
          lines.push(current); current = word;
        } else current = next;
      }
      if (current) lines.push(current);
      const top = TH * .768 - (lines.length - 1) * 50;
      lines.forEach((text, i) => line(parts, text, 89, top + i * 100));
      const geometry = mergeGeometries(parts, false);
      parts.forEach(part => part.dispose());

      const stock = new THREE.Color(item.color).convertLinearToSRGB();
      const light = .2126 * stock.r + .7152 * stock.g + .0722 * stock.b > 120 / 255;
      if (!materials.has(light)) materials.set(light, new THREE.MeshPhysicalMaterial({
        // Bronze on light stock, champagne on blue: glossy but still legible.
        color: light ? '#342419' : '#ecd5a2',
        metalness: .72, roughness: .22, envMapIntensity: 1.15,
        clearcoat: .8, clearcoatRoughness: .16,
      }));
      const mesh = new THREE.Mesh(geometry, materials.get(light));
      mesh.name = 'Raised polished lettering';
      return mesh;
    },
    disposeTemplates() {
      cache.forEach(geometry => geometry.dispose());
      cache.clear();
    },
  };
}
