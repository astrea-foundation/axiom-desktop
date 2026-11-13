import { SVGLoader } from 'three/addons/loaders/SVGLoader.js';
import { mergeGeometries, mergeVertices } from 'three/addons/utils/BufferGeometryUtils.js';
import { createLetteringFactory } from './proof-lettering.js';
import { createFoilStroke } from './proof-foil.js';

// Millimetre-scale details are geometry; the paper's fibres live in its PBR maps.
// Every card is 2.8 × 4.4 world units, centred at the origin, with its face on +Z.
export async function createProofCards(THREE, renderer, data) {
  const W = 2.8, H = 4.4, D = 0.045, FACE = D / 2;
  const TW = 1536, TH = Math.round(TW * H / W);
  const anisotropy = Math.min(8, renderer.capabilities.getMaxAnisotropy());
  const yieldToPage = () => globalThis.scheduler?.yield
    ? globalThis.scheduler.yield()
    : new Promise(resolve => setTimeout(resolve, 0));
  const loader = new SVGLoader();
  const [, svgTexts, lettering] = await Promise.all([
    Promise.all([document.fonts.load('400 90px "PP Cirka"'), document.fonts.load('400 40px "DM Sans"')]),
    Promise.all(data.map(async ({ art }) => {
      const response = await fetch(art);
      if (!response.ok) throw new Error(`Could not load card engraving: ${art}`);
      return response.text();
    })),
    createLetteringFactory(THREE),
  ]);

  function random(seed) {
    return () => {
      seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
      let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }
  function canvas(w, h) {
    const c = document.createElement('canvas'); c.width = w; c.height = h;
    return c;
  }
  function texture(c, color = false) {
    const t = new THREE.CanvasTexture(c);
    t.colorSpace = color ? THREE.SRGBColorSpace : THREE.NoColorSpace;
    t.anisotropy = anisotropy;
    return t;
  }
  function roundedShape(w, h, radius) {
    const s = new THREE.Shape(), x = -w / 2, y = -h / 2;
    s.moveTo(x + radius, y); s.lineTo(x + w - radius, y);
    s.quadraticCurveTo(x + w, y, x + w, y + radius);
    s.lineTo(x + w, y + h - radius);
    s.quadraticCurveTo(x + w, y + h, x + w - radius, y + h);
    s.lineTo(x + radius, y + h);
    s.quadraticCurveTo(x, y + h, x, y + h - radius);
    s.lineTo(x, y + radius); s.quadraticCurveTo(x, y, x + radius, y);
    return s;
  }
  function faceUV(geometry) {
    const p = geometry.attributes.position, uv = geometry.attributes.uv;
    for (let i = 0; i < p.count; i++) uv.setXY(i, p.getX(i) / W + 0.5, p.getY(i) / H + 0.5);
    return geometry;
  }

  // Short, randomly oriented pulp fibres over broad, irregular cotton clumps.
  // The four octaves avoid a regular textile weave or a uniform noise overlay.
  async function paperSurface() {
    const w = 768, h = Math.round(w * H / W), rand = random(170991);
    const height = canvas(w, h), hc = height.getContext('2d');
    const fields = [8, 29, 113].map(n => {
      const ny = Math.ceil(n * h / w), values = new Float32Array((n + 1) * (ny + 1));
      for (let i = 0; i < values.length; i++) values[i] = rand();
      return { n, ny, values };
    });
    function sample(f, x, y) {
      const gx = x / w * f.n, gy = y / h * f.ny;
      const ix = Math.floor(gx), iy = Math.floor(gy);
      let u = gx - ix, v = gy - iy;
      u = u * u * (3 - 2 * u); v = v * v * (3 - 2 * v);
      const k = iy * (f.n + 1) + ix, a = f.values;
      return (a[k] * (1-u) + a[k+1] * u) * (1-v)
        + (a[k+f.n+1] * (1-u) + a[k+f.n+2] * u) * v;
    }
    const pixels = hc.createImageData(w, h);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const n = 0.17 * sample(fields[0], x, y) + 0.28 * sample(fields[1], x, y)
          + 0.36 * sample(fields[2], x, y) + 0.19 * rand();
        const k = (y * w + x) * 4, value = Math.round(40 + n * 174);
        pixels.data[k] = pixels.data[k+1] = pixels.data[k+2] = value;
        pixels.data[k+3] = 255;
      }
      if ((y + 1) % 256 === 0) await yieldToPage();
    }
    hc.putImageData(pixels, 0, 0);
    hc.lineCap = 'round';
    for (let i = 0; i < 22000; i++) {
      const x = rand() * w, y = rand() * h, a = rand() * Math.PI * 2;
      const length = 0.7 + Math.pow(rand(), 3) * 9;
      hc.strokeStyle = rand() > .5 ? 'rgba(242,242,242,.23)' : 'rgba(25,25,25,.16)';
      hc.lineWidth = .35 + rand() * .8;
      hc.beginPath(); hc.moveTo(x, y);
      hc.quadraticCurveTo(x + Math.cos(a+.2)*length*.5, y + Math.sin(a+.2)*length*.5,
        x + Math.cos(a)*length, y + Math.sin(a)*length);
      hc.stroke();
      if ((i + 1) % 5000 === 0) await yieldToPage();
    }
    const source = hc.getImageData(0, 0, w, h).data;
    const normal = canvas(w, h), nc = normal.getContext('2d'), ni = nc.createImageData(w, h);
    const rough = canvas(w, h), rc = rough.getContext('2d'), ri = rc.createImageData(w, h);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const k = (y*w+x)*4;
        const dx = (source[(y*w+Math.max(0,x-1))*4] - source[(y*w+Math.min(w-1,x+1))*4]) / 255;
        const dy = (source[(Math.max(0,y-1)*w+x)*4] - source[(Math.min(h-1,y+1)*w+x)*4]) / 255;
        const z = 1 / Math.sqrt(dx*dx*8 + dy*dy*8 + 1);
        ni.data[k] = 128 + dx*2.8*z*127;
        ni.data[k+1] = 128 - dy*2.8*z*127;
        ni.data[k+2] = z*127+128; ni.data[k+3] = 255;
        const r = 200 + source[k] * .2;
        ri.data[k] = ri.data[k+1] = ri.data[k+2] = r; ri.data[k+3] = 255;
      }
      if ((y + 1) % 256 === 0) await yieldToPage();
    }
    nc.putImageData(ni, 0, 0); rc.putImageData(ri, 0, 0);
    return { height, pixels: source, w, h, normal: texture(normal), roughness: texture(rough), bump: texture(height) };
  }
  const paper = await paperSurface();
  await yieldToPage();

  const bodyGeometry = new THREE.ExtrudeGeometry(roundedShape(W-.009, H-.009, .085), {
    depth: D-.014, steps: 1, bevelEnabled: true, bevelThickness: .007,
    bevelSize: .0045, bevelSegments: 3, curveSegments: 8,
  });
  bodyGeometry.translate(0, 0, -(D-.014)/2);
  const faceGeometry = faceUV(new THREE.ShapeGeometry(roundedShape(W-.013, H-.013, .08), 12));
  const backGeometry = faceGeometry.clone();
  backGeometry.rotateY(Math.PI);

  const edgeMaterial = new THREE.MeshStandardMaterial({
    color: '#d6cbb2', roughness: .98, metalness: 0,
    bumpMap: paper.bump, bumpScale: .009,
  });
  const gold = new THREE.MeshPhysicalMaterial({
    color: '#d4a047', metalness: 1, roughness: .22, envMapIntensity: 1.4,
    normalMap: paper.normal, normalScale: new THREE.Vector2(.025,.025),
    clearcoat: .22, clearcoatRoughness: .2,
  });

  function merge(parts) {
    // Keep the foil's shared vertices instead of expanding every triangle.
    // Extrusions preserve their distinct bevel normals when they are indexed.
    const compatible = parts.map(g => g.index ? g : mergeVertices(g, 1e-7));
    const result = mergeGeometries(compatible, false);
    for (const g of new Set([...parts, ...compatible])) g.dispose();
    return result;
  }
  function tube(path, halfWidth = .0105, segments = 64) {
    const points = [];
    for (let i = 0; i <= segments; i++) {
      const t = i / segments, p = path.getPoint(t), tangent = path.getTangent(t);
      points.push({ x: p.x, y: p.y, nx: tangent.y, ny: -tangent.x });
    }
    return createFoilStroke(THREE, points, halfWidth,
      { face: FACE, cardWidth: W, cardHeight: H });
  }
  function bezier(points, halfWidth = .0115) {
    const path = new THREE.Path(); path.moveTo(points[0], points[1]);
    for (let i = 2; i < points.length; i += 6) path.bezierCurveTo(...points.slice(i, i+6));
    return tube(path, halfWidth);
  }
  function leaf(x, y, rotation, length = .105) {
    const s = new THREE.Shape(); s.moveTo(0, 0);
    s.bezierCurveTo(-length*.35, length*.36, -length*.26, length*.7, 0, length);
    s.bezierCurveTo(length*.22, length*.65, length*.31, length*.28, 0, 0);
    const g = new THREE.ExtrudeGeometry(s, { depth: .006, bevelEnabled: true,
      bevelThickness: .003, bevelSize: .002, bevelSegments: 1, curveSegments: 5 });
    g.rotateZ(rotation); g.translate(x, y, FACE+.008); return g;
  }

  function roundedBorder(w, h, cornerRadius, halfWidth) {
    // Sample each circular corner directly. Distributing a tube's samples over
    // the whole perimeter leaves too few at these small, highly visible turns.
    const points = [], cornerSegments = 24;
    const cx = w/2-cornerRadius, cy = h/2-cornerRadius;
    const corners = [[cx,-cy,-Math.PI/2], [cx,cy,0],
      [-cx,cy,Math.PI/2], [-cx,-cy,Math.PI]];
    for (const [x,y,start] of corners) for (let i = 0; i <= cornerSegments; i++) {
      const angle = start + i/cornerSegments * Math.PI/2;
      const nx = Math.cos(angle), ny = Math.sin(angle);
      points.push({ x: x+nx*cornerRadius, y: y+ny*cornerRadius, nx, ny });
    }
    return createFoilStroke(THREE, points, halfWidth,
      { face: FACE, cardWidth: W, cardHeight: H, closed: true });
  }

  // The frame, flourishes and leaves share a broad stamped-foil highlight.
  const ornamentParts = [];
  ornamentParts.push(roundedBorder(W-.25,H-.25,.15,.0115));
  const corner = [
    bezier([0, .52, .03,.37, .03,.13, .18,.1, .39,.04, .34,.36, .19,.32,
      .09,.29, .17,.17, .22,.23]),
    bezier([.02,.12, .1,.19, .24,.06, .45,.025, .56,.01, .58,.14, .49,.16,
      .41,.18, .42,.08, .48,.105], .0105),
    bezier([.07,.44, .1,.34, .2,.39, .23,.45, .27,.55, .16,.57, .155,.49], .0095),
    leaf(.035,.31, -.4), leaf(.35,.05, -1.1), leaf(.105,.395,.65,.083),
  ];
  const cornerGeometry = merge(corner);
  for (const sx of [-1, 1]) for (const sy of [-1, 1]) {
    const c = cornerGeometry.clone();
    // Mirror with a rotation, then correct the winding when only one axis flips.
    c.scale(sx, sy, 1);
    if (sx * sy < 0) {
      const indices = c.index.array;
      for (let i = 0; i < indices.length; i += 3) {
        const second = indices[i + 1]; indices[i + 1] = indices[i + 2]; indices[i + 2] = second;
      }
    }
    c.translate(-sx*(W/2-.18), -sy*(H/2-.18), 0);
    ornamentParts.push(c);
  }
  cornerGeometry.dispose();
  // A small hand-set ornament separates illustration from explanatory copy.
  const separator = new THREE.Shape();
  separator.moveTo(0,-.60); separator.lineTo(.026,-.638);
  separator.lineTo(0,-.676); separator.lineTo(-.026,-.638); separator.closePath();
  const separatorGeometry = new THREE.ExtrudeGeometry(separator, {
    depth: .004, bevelEnabled: true, bevelThickness: .002, bevelSize: .002, bevelSegments: 1,
  });
  separatorGeometry.translate(0,0,FACE+.007); ornamentParts.push(separatorGeometry);
  const ornamentGeometry = faceUV(merge(ornamentParts));

  function tracked(ctx, text, x, y, tracking) {
    const chars = [...text], widths = chars.map(c => ctx.measureText(c).width);
    let cursor = x - (widths.reduce((a,b) => a+b, 0) + tracking*(chars.length-1))/2;
    chars.forEach((c, i) => { ctx.fillText(c, cursor, y); cursor += widths[i] + tracking; });
  }
  function tintedPaper(item, index) {
    const grainCanvas = canvas(paper.w, paper.h), gc = grainCanvas.getContext('2d');
    const color = new THREE.Color(item.color || ['#bd6851','#3e6285','#cdbc96','#6b816b'][index]);
    color.convertLinearToSRGB();
    const base = [color.r*255,color.g*255,color.b*255];
    const image = gc.createImageData(paper.w, paper.h);
    for (let i = 0; i < paper.pixels.length; i += 4) {
      const variation = (paper.pixels[i]-126)*.13;
      image.data[i] = base[0]+variation; image.data[i+1] = base[1]+variation*.89;
      image.data[i+2] = base[2]+variation*.72; image.data[i+3] = 255;
    }
    gc.putImageData(image,0,0);
    return { grainCanvas, base };
  }
  function printCanvas({ grainCanvas, base }, backside = false) {
    // Main lettering is geometry; a 1024px stock map still exceeds the native
    // fibre resolution while avoiding an oversized, mostly empty colour map.
    const resolution = backside ? .5 : 2 / 3;
    const c = canvas(Math.round(TW*resolution), Math.round(TH*resolution)), ctx = c.getContext('2d');
    ctx.scale(resolution,resolution);
    ctx.drawImage(grainCanvas,0,0,TW,TH);
    // Subtle compression at the trimmed edge and inside the foil border.
    const edge = ctx.createLinearGradient(0,0,TW,TH);
    edge.addColorStop(0,'rgba(255,241,212,.06)');
    edge.addColorStop(.46,'rgba(0,0,0,0)'); edge.addColorStop(1,'rgba(40,25,15,.075)');
    ctx.fillStyle = edge; ctx.fillRect(0,0,TW,TH);
    // Studio illumination lifts the stock; dark letterpress ink keeps the
    // coral, parchment and sage cards readable at their mobile display size.
    const light = .2126*base[0]+.7152*base[1]+.0722*base[2] > 120;
    ctx.fillStyle = light ? '#342a21' : '#fff3d9';
    if (backside) {
      ctx.font = '400 48px "DM Sans", sans-serif'; ctx.textAlign = 'left';
      tracked(ctx,'AXIOMAI',TW/2,TH*.22,13);
      ctx.font = '400 180px "PP Cirka", serif'; ctx.textAlign = 'center';
      ctx.fillText('A',TW/2,TH*.56);
      ctx.font = '400 35px "DM Sans", sans-serif'; ctx.textAlign = 'left';
      tracked(ctx,'PRIVATELY HELD',TW/2,TH*.8,7);
    } else {
      // The title, numeral and body copy are separate raised lettering meshes.
      ctx.font = '400 34px "DM Sans", sans-serif'; ctx.textAlign = 'left';
      tracked(ctx,'AXM',TW*.32,TH*.935,5);
      tracked(ctx,'MXXIV',TW*.68,TH*.935,5);
    }
    return c;
  }

  function engraving(svgText) {
    const parsed = loader.parse(svgText), shapes = [];
    for (const path of parsed.paths) shapes.push(...SVGLoader.createShapes(path));
    const flat = new THREE.ShapeGeometry(shapes, 3); flat.computeBoundingBox();
    const box = flat.boundingBox, size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3()); flat.dispose();
    const scale = Math.min(1.67/size.x, 1.74/size.y);
    const embossed = new THREE.ExtrudeGeometry(shapes, {
      depth: .007/scale, steps: 1, curveSegments: 3,
      bevelEnabled: true, bevelThickness: .0035/scale,
      bevelSize: .0025/scale, bevelSegments: 2,
    });
    // Rotation reverses SVG Y while preserving the winding and correct normals.
    embossed.rotateX(Math.PI); embossed.scale(scale,scale,scale);
    embossed.translate(-center.x*scale,center.y*scale+.32,FACE+.012);
    return faceUV(embossed);
  }

  const cards = [];
  for (const [index, item] of data.entries()) {
    await yieldToPage();
    const group = new THREE.Group(); group.name = `Proof card — ${item.name}`;
    group.userData = { width: W, height: H, thickness: D, ...item };
    const substrate = new THREE.Mesh(bodyGeometry,edgeMaterial); substrate.name = 'Cotton paper edge';
    group.add(substrate);
    const stock = tintedPaper(item,index);
    for (const backside of [false,true]) {
      const material = new THREE.MeshPhysicalMaterial({
        map: texture(printCanvas(stock,backside),true),
        roughness: .93, roughnessMap: paper.roughness, metalness: 0,
        normalMap: paper.normal, normalScale: new THREE.Vector2(.42,.42),
        bumpMap: paper.bump, bumpScale: .009, envMapIntensity: .65,
        // A very thin satin sizing catches broad studio light while the
        // cotton fibres remain visible through both reflection layers.
        clearcoat: .13, clearcoatRoughness: .6,
        clearcoatNormalMap: paper.normal,
        clearcoatNormalScale: new THREE.Vector2(.3,.3),
        sheen: .1, sheenColor: '#f7e7cc', sheenRoughness: .85,
      });
      const face = new THREE.Mesh(backside ? backGeometry : faceGeometry,material);
      face.position.z = backside ? -FACE-.0005 : FACE+.0005;
      face.name = backside ? 'Printed reverse' : 'Letterpress cotton face'; group.add(face);
    }
    await yieldToPage();
    const filigree = new THREE.Mesh(ornamentGeometry,gold); filigree.name = 'Raised gold corner flourishes';
    group.add(filigree);
    const symbol = new THREE.Mesh(engraving(svgTexts[index]),gold); symbol.name = 'Bevelled gold engraving';
    group.add(symbol);
    await yieldToPage();
    group.add(lettering.create(item));
    group.traverse(child => { if (child.isMesh) { child.castShadow = true; child.receiveShadow = true; } });
    cards.push(group);
  }
  lettering.disposeTemplates();
  return cards;
}
