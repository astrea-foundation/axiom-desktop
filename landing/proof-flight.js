// A close, centered presentation precedes one continuous retreat into a fixed
// slot. The next approach begins while the preceding card is settling.
export const FLIGHT_OFFSETS = [0, 1, 2, 3];
export const FLIGHT_START = -1.08;
export const FLIGHT_PRESENT_START = 0;
export const FLIGHT_PRESENT_END = .36;
export const FLIGHT_ARRIVAL = 1.04;
export const FLIGHT_FEATURE_DEPTH = 2.6;
export const FLIGHT_SPAN = FLIGHT_OFFSETS.at(-1) + FLIGHT_ARRIVAL + .56 - FLIGHT_START;
const HALF_FOV = Math.tan(Math.PI / 10);
const CARD_ASPECT = 2.8 / 4.4;

// Height is the canvas below the pinned heading, not the full sticky stage.
// Sharing this framing keeps the renderer and physical checks in agreement.
export function createProofFraming(width, height) {
  const aspect = width / height;
  const stacked = width <= 700;
  const narrow = (width < 760 && aspect < 1.1) || aspect < .9;
  const cameraZ = 4.4 / (2 * HALF_FOV * .78);
  const halfHeight = cameraZ * HALF_FOV;
  const cardHalfHeight = stacked
    ? Math.min(.80, .80 * aspect / CARD_ASPECT)
    : narrow ? Math.min(.415, .405 * aspect / CARD_ASPECT)
      : Math.min(.78, .204 * aspect / CARD_ASPECT);
  const cardScale = cardHalfHeight * halfHeight / 2.2;
  const featuredHalfHeight = Math.min(.9, .86 * aspect / CARD_ASPECT);
  const featuredScale = featuredHalfHeight * (cameraZ - FLIGHT_FEATURE_DEPTH) * HALF_FOV / 2.2;
  const stackStep = cardHalfHeight * 2 + .18;
  const stackTravel = stacked ? stackStep * 3 * height / 2 : 0;
  const stackEase = stacked ? Math.min(64, stackTravel * .08) : 0;
  return { aspect, narrow, stacked, cardScale, featuredScale, cameraZ, halfHeight,
    stackStep, stackTravel, stackEase, stackScroll: stackTravel + stackEase, height };
}

export function createFlight(index, framing) {
  const { narrow, stacked, stackStep, cardScale, featuredScale } = framing;
  const x = stacked ? 0 : narrow ? (index % 2 ? .46 : -.46) : (index - 1.5) * .444;
  const y = stacked ? -index * stackStep : narrow ? (index < 2 ? .455 : -.455) : 0;
  const [side, rise, roll] = [[1, -1, -.5], [-1, 1, .42], [1, 1, -.32], [-1, -1, .55]][index];
  const final = [x, y, 0, 0, 0, 0, cardScale];
  const featured = [0, 0, FLIGHT_FEATURE_DEPTH, 0, 0, 0, featuredScale];
  // [time, screen-normalized x/y, world depth, pitch/yaw/roll, world scale]
  // The off-screen bend reaches the foreground before crossing the settled
  // layout. Retreat aligns with its own slot before reaching that layout's z=0.
  const keys = [
    [-1.12, side * 2.2, rise * 2.1, -10, rise * .42, -side * 1.25, roll, featuredScale],
    [-.67, side * 1.85, rise * 1.8, 3.4, -rise * .28, side * .95, roll * .6, featuredScale],
    [-.22, side * .075, rise * .035, 2.8, -rise * .045, side * .09, roll * .07, featuredScale],
    [FLIGHT_PRESENT_START, ...featured],
    [FLIGHT_PRESENT_END, ...featured],
    [.78, x, y, 1.1, 0, 0, 0, featuredScale + (cardScale - featuredScale) * .67],
    [FLIGHT_ARRIVAL, ...final],
  ];
  const tangents = keys.map((key, i) => {
    const before = keys[Math.max(0, i - 1)];
    const after = keys[Math.min(keys.length - 1, i + 1)];
    return key.slice(1).map((_, j) => (after[j + 1] - before[j + 1]) / (after[0] - before[0]));
  });
  // Ease into a genuinely still, readable face. Arrivals are scroll-driven,
  // so stopping here leaves the card perfectly sharp for as long as needed.
  tangents[2] = [-side * .45, -rise * .2, -.6, rise * .2, -side * .4, -roll * .3, 0];
  tangents[3] = tangents[4] = new Array(7).fill(0);
  // Finish screen-space travel before the final depth approach. This prevents
  // a card from slicing across a neighbour as it joins the shared final plane.
  tangents[5][0] = tangents[5][1] = 0;
  tangents[6] = new Array(7).fill(0);
  return { index, keys, tangents, final, featured, narrow, stacked };
}

export function sampleFlight(flight, time, out) {
  const { keys, tangents, final } = flight;
  out.visible = time > keys[0][0];
  if (time >= FLIGHT_ARRIVAL) {
    out.pose.set(final);
    return out;
  }
  const t = Math.max(keys[0][0], time);
  let segment = 0;
  while (segment < keys.length - 2 && t > keys[segment + 1][0]) segment++;
  const a = keys[segment], b = keys[segment + 1];
  const duration = b[0] - a[0], u = (t - a[0]) / duration;
  const u2 = u * u, u3 = u2 * u;
  const h00 = 2 * u3 - 3 * u2 + 1;
  const h10 = u3 - 2 * u2 + u;
  const h01 = -2 * u3 + 3 * u2;
  const h11 = u3 - u2;
  for (let i = 0; i < 7; i++) {
    out.pose[i] = h00 * a[i + 1] + h10 * duration * tangents[segment][i]
      + h01 * b[i + 1] + h11 * duration * tangents[segment + 1][i];
  }
  return out;
}

// Keep every presentation on the same sharp foreground plane, then rack focus
// with the last retreat so the entire completed row/grid is sharply readable.
export function sampleProofFocus(flights, position, scratch) {
  const lastTime = position - FLIGHT_OFFSETS.at(-1);
  if (lastTime <= FLIGHT_PRESENT_END) return FLIGHT_FEATURE_DEPTH;
  return sampleFlight(flights.at(-1), lastTime, scratch).pose[2];
}

// After collection, phones scroll through a fixed, full-size vertical column.
// The central stretch maps one CSS scroll pixel to one projected pixel; only
// the first/last 64 pixels ease into and out of the camera's vertical travel.
export function sampleProofScroll(framing, distance, flightLength, out) {
  out.position = FLIGHT_START + Math.max(0, Math.min(1, distance / flightLength)) * FLIGHT_SPAN;
  out.cameraY = 0;
  if (!framing.stacked) return out;
  const { stackTravel, stackEase, stackScroll, halfHeight, height } = framing;
  const d = Math.max(0, Math.min(stackScroll, distance - flightLength));
  const pan = d < stackEase ? d * d / (2 * stackEase)
    : d < stackTravel ? d - stackEase / 2
      : stackTravel - (stackScroll - d) ** 2 / (2 * stackEase);
  out.cameraY = pan > 0 ? -pan * 2 * halfHeight / height : 0;
  return out;
}
