// A pressed foil stroke has a reflective crown and softly rolled shoulders.
// Unlike a round wire, its front occupies enough pixels to catch the same
// broad studio lights as the stamped leaves and central engraving.
export function createFoilStroke(THREE, points, halfWidth, {
  face, cardWidth, cardHeight, closed = false,
}) {
  const crown = halfWidth * .55, shoulder = halfWidth - crown;
  const base = face + .003, shoulderZ = face + .011, top = face + .016;
  const profile = [[halfWidth, base, 1, 0]];
  const shoulderSteps = 6;
  for (let i = 0; i <= shoulderSteps; i++) {
    const angle = i / shoulderSteps * Math.PI / 2;
    const x = Math.cos(angle), y = Math.sin(angle);
    const nx = x / shoulder, nz = y / (top - shoulderZ);
    const length = Math.hypot(nx, nz);
    profile.push([crown + shoulder * x, shoulderZ + (top - shoulderZ) * y,
      nx / length, nz / length]);
  }
  for (let i = 0; i <= shoulderSteps; i++) {
    const angle = Math.PI / 2 + i / shoulderSteps * Math.PI / 2;
    const x = Math.cos(angle), y = Math.sin(angle);
    const nx = x / shoulder, nz = y / (top - shoulderZ);
    const length = Math.hypot(nx, nz);
    profile.push([-crown + shoulder * x, shoulderZ + (top - shoulderZ) * y,
      nx / length, nz / length]);
  }
  profile.push([-halfWidth, base, -1, 0], [-halfWidth, base, 0, -1],
    [halfWidth, base, 0, -1], [halfWidth, base, 1, 0]);

  const positions = [], normals = [], uvs = [], indices = [];
  const stride = profile.length;
  function vertex(x, y, z, nx, ny, nz) {
    positions.push(x, y, z); normals.push(nx, ny, nz);
    // The paper microstructure stays in the card's plane, including on foil.
    uvs.push(x / cardWidth + .5, y / cardHeight + .5);
  }
  points.forEach(p => {
    for (const [offset, z, nx, nz] of profile) {
      vertex(p.x + p.nx * offset, p.y + p.ny * offset, z,
        p.nx * nx, p.ny * nx, nz);
    }
  });
  const count = closed ? points.length : points.length - 1;
  for (let i = 0; i < count; i++) for (let j = 0; j < stride - 1; j++) {
    // Duplicate lower corners carry separate side/underside normals.
    if (profile[j][0] === profile[j + 1][0] && profile[j][1] === profile[j + 1][1]) continue;
    const a = i * stride + j, b = ((i + 1) % points.length) * stride + j;
    indices.push(a, b, b + 1, a, b + 1, a + 1);
  }
  if (!closed) for (const end of [0, points.length - 1]) {
    const p = points[end], start = end === 0;
    const nx = start ? p.ny : -p.ny, ny = start ? -p.nx : p.nx;
    const center = positions.length / 3;
    vertex(p.x, p.y, (base + top) / 2, nx, ny, 0);
    for (const [offset, z] of profile) {
      vertex(p.x + p.nx * offset, p.y + p.ny * offset, z, nx, ny, 0);
    }
    for (let j = 0; j < stride - 1; j++) {
      if (profile[j][0] === profile[j + 1][0] && profile[j][1] === profile[j + 1][1]) continue;
      const a = center + 1 + j, b = a + 1;
      indices.push(center, start ? a : b, start ? b : a);
    }
  }
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3));
  geometry.setAttribute('normal', new THREE.Float32BufferAttribute(normals, 3));
  geometry.setAttribute('uv', new THREE.Float32BufferAttribute(uvs, 2));
  geometry.setIndex(indices);
  return geometry;
}
