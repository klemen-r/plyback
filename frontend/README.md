# Plyback web interface

React, TypeScript, Vite, Tailwind, TanStack Query and chessground. Fonts and runtime assets are self-hosted.

From this directory, with Node.js 22.12 or newer:

```sh
npm ci
npm run dev       # proxy /api to the backend on localhost:8000
npm run lint
npm test
npm run build
npx playwright install chromium
npm run e2e
```

Run the Python backend separately for development. Override the dev proxy with `API_URL`.
Playwright starts isolated backend/Chess.com fixture servers on 8790/8799 and uses a disposable database.
Tests exercise the built interface; run `npm run build` first.

The final visual direction is pending. A, D, A+D and C remain unapproved; archived design studies live
in [docs/design-explorations](../docs/design-explorations/). Keep feature work independent of that choice.
