The landing is a static site with a small JavaScript build.

```sh
npm ci
npm run build
python3 -m http.server 4173 --bind 127.0.0.1
```

Rebuild after changing JavaScript. The build shares one pinned Three.js runtime
between `landing.js` and the deferred proof chapter, generates content-hashed
scripts in `dist/`, updates their references in `index.html`, and assembles the
public files in `.site/`. Vercel runs the same build and publishes `.site/`.
The scene stylesheet also gets a content version so an open preview loads its
matching layout after a rebuild.

Each card sweeps in from a different corner and holds large, centered and sharp
before retreating directly into its permanent slot. The next approach starts
while the previous card settles, with one timeline unit between presentations.
The readable hold spans local time 0–0.36; placement finishes at 1.04. There is
no intermediate miniature row or later group rearrangement. Desktop uses one
row; portrait tablets keep two rows. At widths up to 700px, final cards form one
full-size vertical column. Placed cards never move again. After collection,
continued native scrolling moves the camera down the phone column so all four
cards can be read in order, then the last card scrolls out with the section.
The canvas remains viewport-sized throughout; the scene never allocates a tall
render target. Its central scroll stretch moves one projected pixel per CSS
scroll pixel, with 64px easing at the beginning and end.
The existing “How it works:” heading stays pinned above a separately sized
canvas, so the cards cannot cover it. `proof-flight.js` shares framing and the
seven pose values (position, rotation, scale) with the geometry checks. Arrivals
reach the foreground while off screen, and retreats align with their own slots
before reaching the final plane. A fixed foreground focus plane keeps every
presentation readable; the lens then follows the last retreat to focus the
complete row or column. Desktop scroll timing is unchanged. Mobile adds the
column's projected first-to-last distance plus 64px to its existing 350svh
track using `--proof-stack-scroll`; the original flight pace is preserved.
The profiler's existing measured track span includes this added scroll-through.

The card scene renders only while its scroll pose is changing. Depth and cast
shadows use the solid paper silhouette; the gold and lettering remain detailed
geometry in the colour pass. Packed depth uses RGBA8 so half-float conversion
cannot shift the focus plane on mobile; HDR colour stays half-float.
Depth-of-field blur runs at half width and height,
while the focused card is composited from the full-resolution image. Preserve
premultiplied alpha through filtering and unpremultiply only in the composite.
Retina render resolution has a pixel budget and steps down if sustained scroll
frames are slow. The selected resolution stays stable when reading, avoiding
buffer reallocations on every wheel gesture.

For local performance comparisons, open `index.html?profile`, scroll to the
cards, and click **Run card performance sweep**. It drives the same native
scroll timeline for 12 seconds and reports frame intervals, draw calls and
triangles. Compare the same viewport on the same device, with only one scene
running. `scripts/proof-profile.js` and its controls are excluded from deployment.
The frame counter should stop after scrolling settles.

```sh
node scripts/check-proof-flight.mjs
```

This checks physical intersections, unobstructed large centered presentations,
focus during reading, one continuous reduction into the final size, exact fixed
poses after arrival, smooth segment boundaries, and the final sharp, fully
visible layout across desktop and mobile canvas sizes. Phone checks additionally
verify a fixed single column, monotonic camera travel, and a fully visible
reading interval for every card before sticky release. Use `index.html?static` to
check the accessible card fallback.
