import assert from 'node:assert/strict';
import { createProofFraming, createFlight, sampleFlight, sampleProofFocus, sampleProofScroll, FLIGHT_OFFSETS, FLIGHT_START, FLIGHT_SPAN, FLIGHT_ARRIVAL, FLIGHT_PRESENT_START, FLIGHT_PRESENT_END } from '../proof-flight.js';

// Conservative boxes contain the paper, embossed lettering and gold relief.
// Separating-axis checks catch real intersections, including during a turn.
const halfSize = [1.4, 2.2, .045];
const halfFov = Math.tan(Math.PI / 10);
const dot = (a, b) => a.reduce((sum, value, i) => sum + value * b[i], 0);
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];

function cardBox(flight, phase, aspect, cameraZ) {
  const sample = sampleFlight(flight, phase, { pose: new Float64Array(7) });
  if (!sample.visible) return null;
  assert.ok([...sample.pose].every(Number.isFinite), 'Flight must contain finite transforms.');
  const [x, y, z, pitch, yaw, roll, scale] = sample.pose;
  const a = Math.cos(pitch), b = Math.sin(pitch);
  const c = Math.cos(yaw), d = Math.sin(yaw);
  const e = Math.cos(roll), f = Math.sin(roll);
  const height = (cameraZ - z) * halfFov;
  return {
    center: [x * height * aspect, y * height, z],
    scale,
    phase,
    // Columns of Three.js's Euler XYZ rotation matrix.
    axes: [[c * e, a * f + b * e * d, b * f - a * e * d],
      [-c * f, a * e - b * f * d, b * e + a * f * d], [d, -b * c, a * c]],
  };
}

function clearance(a, b) {
  const delta = b.center.map((value, i) => value - a.center[i]);
  const candidates = [...a.axes, ...b.axes, ...a.axes.flatMap(x => b.axes.map(y => cross(x, y)))];
  let separation = -Infinity;
  for (const axis of candidates) {
    const length = Math.hypot(...axis);
    if (length < 1e-8) continue;
    const normal = axis.map(value => value / length);
    const radius = box => halfSize.reduce((sum, size, i) => sum + size * box.scale * Math.abs(dot(normal, box.axes[i])), 0);
    separation = Math.max(separation, Math.abs(dot(normal, delta)) - radius(a) - radius(b));
  }
  return separation;
}

function projectedBounds(box, aspect, cameraZ, cameraY = 0) {
  const points = [];
  for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1]) {
    const vertex = box.center.map((value, axis) => value + [x, y, z].reduce((sum, direction, i) =>
      sum + direction * halfSize[i] * box.scale * box.axes[i][axis], 0));
    const halfHeight = (cameraZ - vertex[2]) * halfFov;
    points.push([vertex[0] / (halfHeight * aspect), (vertex[1] - cameraY) / halfHeight]);
  }
  return { left: Math.min(...points.map(p => p[0])), right: Math.max(...points.map(p => p[0])),
    bottom: Math.min(...points.map(p => p[1])), top: Math.max(...points.map(p => p[1])) };
}
function screenGap(a, b) {
  return Math.max(b.left - a.right, a.left - b.right, b.bottom - a.top, a.bottom - b.top);
}

let minimum = Infinity, pairsChecked = 0, heldChecks = 0, finalChecks = 0, readingChecks = 0, stackChecks = 0;
// Canvas sizes include the real space left below the pinned title at the main
// desktop/phone QA sizes, plus a wide aspect range and short-window coverage.
const viewports = [[1280, 528], [1440, 700], [390, 684], [375, 652], [320, 480], [430, 772], [700, 280], [768, 824]];
for (const height of [280, 600, 900]) for (const aspect of [.3, .4, .6, .75, .899, .9, 1, 1.28, 16 / 9, 2.4, 3.6]) {
  viewports.push([height * aspect, height]);
}
for (const [width, height] of viewports) {
  const framing = createProofFraming(width, height);
  const { cameraZ, aspect, narrow, stacked } = framing;
  const flights = FLIGHT_OFFSETS.map((_, i) => createFlight(i, framing));
  for (let phase = FLIGHT_START; phase <= FLIGHT_START + FLIGHT_SPAN; phase += .002) {
    const boxes = flights.map((flight, i) => {
      const localTime = phase - FLIGHT_OFFSETS[i];
      const sample = sampleFlight(flight, localTime, { pose: new Float64Array(7) });
      if (localTime >= FLIGHT_PRESENT_START && localTime <= FLIGHT_PRESENT_END) {
        assert.ok([...sample.pose].every((value, j) => Math.abs(value - flight.featured[j]) < 1e-10), 'Each card must hold a large, centered, front-facing reading pose.');
        const focus = sampleProofFocus(flights, phase, { pose: new Float64Array(7) });
        assert.ok(Math.abs(focus - sample.pose[2]) < 1e-10, 'The presented card must be in focus.');
        const readingBounds = projectedBounds(cardBox(flight, localTime, aspect, cameraZ), aspect, cameraZ);
        const finalBox = cardBox(flight, FLIGHT_ARRIVAL, aspect, cameraZ);
        const finalBounds = projectedBounds(finalBox, aspect, cameraZ, stacked ? finalBox.center[1] : 0);
        assert.ok(readingBounds.top - readingBounds.bottom > (finalBounds.top - finalBounds.bottom) * (stacked ? 1.05 : 1.14), 'The reading card must be visibly larger than its final placement.');
        assert.ok(readingBounds.left > -.94 && readingBounds.right < .94 && readingBounds.bottom > -.94 && readingBounds.top < .94, 'The whole reading card must fit beneath the title.');
        readingChecks++;
      }
      if (localTime >= FLIGHT_ARRIVAL) {
        assert.deepEqual([...sample.pose], flight.final, 'A placed card must stay in its exact final slot as later cards arrive.');
        heldChecks++;
      }
      return cardBox(flight, localTime, aspect, cameraZ);
    });
    for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
      if (!boxes[i] || !boxes[j]) continue;
      const gap = clearance(boxes[i], boxes[j]);
      assert.ok(gap > .05, `Cards ${i + 1}/${j + 1} clash: width=${width}, height=${height}, phase=${phase}, gap=${gap}`);
      minimum = Math.min(minimum, gap);
      pairsChecked++;
      // A later arrival may pass in front of the row, but cannot obscure the
      // card currently being read. Previous cards sit behind the reading plane.
      if (boxes[i].phase >= FLIGHT_PRESENT_START && boxes[i].phase <= FLIGHT_PRESENT_END) {
        const screenClearance = screenGap(projectedBounds(boxes[i], aspect, cameraZ), projectedBounds(boxes[j], aspect, cameraZ));
        assert.ok(screenClearance > .015 || boxes[j].center[2] < boxes[i].center[2] - .5, `The next arrival must leave the current reading face unobstructed: width=${width}, height=${height}, phase=${phase}, pair=${i}/${j}, gap=${screenClearance}.`);
      }
    }
  }
  const end = FLIGHT_START + FLIGHT_SPAN;
  assert.equal(sampleProofFocus(flights, end, { pose: new Float64Array(7) }), 0, 'The complete layout must end in focus.');
  const finalBoxes = flights.map((flight, i) => {
    const phase = end - FLIGHT_OFFSETS[i];
    const sample = sampleFlight(flight, phase, { pose: new Float64Array(7) });
    const held = sampleFlight(flight, phase + 1, { pose: new Float64Array(7) });
    assert.equal(sample.visible, true);
    assert.deepEqual(sample.pose, held.pose, 'The completed layout must persist through sticky release.');
    assert.ok([...sample.pose.slice(2, 6)].every(value => value === 0), 'Final faces must all share the sharp, front-facing plane.');
    const box = cardBox(flight, phase, aspect, cameraZ);
    const bounds = projectedBounds(box, aspect, cameraZ, stacked ? box.center[1] : 0);
    assert.ok(bounds.left > -.97 && bounds.right < .97 && bounds.bottom > -.97 && bounds.top < .97,
      `Final card ${i + 1} clips the canvas: width=${width}, height=${height}, bounds=${JSON.stringify(bounds)}`);
    finalChecks++;
    return { box, bounds: projectedBounds(box, aspect, cameraZ) };
  });
  for (let i = 0; i < 4; i++) for (let j = i + 1; j < 4; j++) {
    assert.ok(screenGap(finalBoxes[i].bounds, finalBoxes[j].bounds) > .025, 'Every final card must have a clear visual gap.');
  }
  const y = finalBoxes.map(({ box }) => box.center[1]);
  if (stacked) assert.ok(finalBoxes.every(({ box }) => box.center[0] === 0) && y.every((value, i) => i === 0 || value < y[i - 1]), 'Phone cards must finish in one ordered vertical column.');
  else if (!narrow) assert.ok(y.every(value => value === y[0]), 'Desktop cards must finish side by side.');
  else assert.ok(y[0] === y[1] && y[2] === y[3] && y[0] > y[2], 'Phone cards must finish in two ordered rows.');
  if (stacked) {
    const flightLength = 3.1 * (height + 160);
    const initial = sampleProofScroll(framing, flightLength, flightLength, {});
    const complete = sampleProofScroll(framing, flightLength + framing.stackScroll, flightLength, {});
    assert.equal(initial.cameraY, 0, 'Column scrolling must start from the first card after collection.');
    assert.ok(Math.abs(complete.cameraY - finalBoxes.at(-1).box.center[1]) < 1e-10, 'The full column must be reachable before sticky release.');
    const visibleCards = new Set();
    let previousY = 0;
    for (let distance = 0; distance <= framing.stackScroll; distance += 1) {
      const sample = sampleProofScroll(framing, flightLength + distance, flightLength, {});
      assert.equal(sample.position, end, 'Card flight poses must remain fixed while scrolling through the column.');
      assert.ok(sample.cameraY <= previousY + 1e-10, 'Scrolling down must pass the column in order.');
      previousY = sample.cameraY;
      for (let i = 0; i < finalBoxes.length; i++) {
        const bounds = projectedBounds(finalBoxes[i].box, aspect, cameraZ, sample.cameraY);
        if (bounds.bottom > -.97 && bounds.top < .97) visibleCards.add(i);
      }
      stackChecks++;
    }
    assert.equal(visibleCards.size, 4, 'Every full-size final card must become fully readable during the column scroll.');
  }
  for (const flight of flights) {
    let previousHeight = Infinity;
    const finalBox = cardBox(flight, FLIGHT_ARRIVAL, aspect, cameraZ);
    const finalBounds = projectedBounds(finalBox, aspect, cameraZ, stacked ? finalBox.center[1] : 0);
    const finalHeight = finalBounds.top - finalBounds.bottom;
    for (let time = FLIGHT_PRESENT_END; time <= FLIGHT_ARRIVAL; time += .002) {
      const box = cardBox(flight, time, aspect, cameraZ);
      const bounds = projectedBounds(box, aspect, cameraZ, stacked ? box.center[1] : 0);
      const height = bounds.top - bounds.bottom;
      assert.ok(height <= previousHeight + 1e-6 && height >= finalHeight - 1e-6, 'A presented card must settle once, without a tiny intermediate pose or later enlargement.');
      previousHeight = height;
    }
  }
  for (const flight of flights) for (const time of flight.keys.map(key => key[0]).slice(1)) {
    const before = sampleFlight(flight, time - 1e-5, { pose: new Float64Array(7) }).pose;
    const after = sampleFlight(flight, time + 1e-5, { pose: new Float64Array(7) }).pose;
    const center = sampleFlight(flight, time, { pose: new Float64Array(7) }).pose;
    assert.ok([...after].every((value, i) => Math.abs(value - before[i]) < .002), 'Flight must remain continuous through arrival.');
    assert.ok([...after].every((value, i) => Math.abs((value - center[i]) - (center[i] - before[i])) / 1e-5 < .04),
      'Flight velocity must ease continuously into its fixed slot.');
  }
}
console.log(`${pairsChecked} card-pair samples passed; minimum clearance ${minimum.toFixed(3)} world units. ${readingChecks} large centered reading checks, ${heldChecks} fixed-slot checks, ${finalChecks} final composition checks, and ${stackChecks} column scroll samples passed.`);
