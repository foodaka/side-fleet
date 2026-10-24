# Sidefleet website

The marketing site: one static page, no build step. `index.html`, `styles.css`,
`fibres.js` (the canvas strands and their travelling pulses) and `page.js` (the
animated stories in each section). Screenshots in `img/` are copies of
`docs/screenshots/`; Inter is the same font file the app ships.

Preview it:

```sh
python3 -m http.server 8765 -d site
```

Live at https://www.sidefleet.xyz (Vercel project `sidefleet`, Stepracers team). Deploy from
this folder with `vercel deploy --prod --scope stepracers --project sidefleet`; `vercel.json`
sets caching and `.vercelignore` keeps the brand sources out. Vercel Web Analytics is on for
the project, with events for the sign-in, install, copy and GitHub clicks.

## Brand

`brand/` holds the mark: the page on the left, fibres fanning out to two machines (cool
to one, warm to the other). `python3 brand/mark.py` regenerates every SVG from one
definition: `mark.svg` (adapts to light and dark), `mark-light.svg` / `mark-dark.svg`,
`mark-small.svg` (three strands, for 32px and below), `mark-animated.svg` (pulses run to
each machine in turn), `app-icon.svg`, `app-icon-small.svg` (for favicons) and `maskable.svg`
(Android's full-bleed icon). `node brand/render.mjs` renders the PNGs, `favicon.ico` and
the social card `og.png` (from `og.html`). The app serves copies from `web/static/`.
