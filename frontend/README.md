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

For development, run the server (`just run`) in another terminal; the dev server (`just dev`) proxies `/api` to
http://127.0.0.1:8000. Override that with `API_URL`.
Playwright starts isolated backend/Chess.com fixture servers on 8790/8799 and uses a disposable database.
Tests exercise the built interface; run `npm run build` first.

The interface uses a breadcrumb header, underlined tabs, bordered cards and stat grids, with sentence case
and weight 600. Keep the Ctrl/Command+K palette and reuse the primitives in `src/components/ui.tsx`.
