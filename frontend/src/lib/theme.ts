import { useEffect, useSyncExternalStore } from 'react'
function markSvg(accent: string, tile = '#171717', fg = '#ffffff'): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="${tile}"/>`
    + `<g transform="translate(5.5 4.5) scale(.86)"><rect x="10" y="4" width="17" height="17" rx="2.5" fill="${fg}"/>`
    + `<rect x="10" y="24" width="17" height="17" rx="2.5" fill="${fg}" opacity=".5"/>`
    + `<rect x="10" y="44" width="17" height="17" rx="2.5" fill="${fg}" opacity=".25"/>`
    + `<rect x="31" y="4" width="17" height="17" rx="2.5" fill="${accent}"/></g></svg>`
}



export type Mode = 'system' | 'light' | 'dark'
export type BoardTheme = 'slate' | 'walnut' | 'green' | 'mono'

export interface Appearance {
  mode: Mode
  accent: string
  board: BoardTheme
}

export const DEFAULT_ACCENT = '#244b84'
export const ACCENT_PRESETS = [
  { name: 'Informant blue', value: '#244b84' },
  { name: 'Ink', value: '#171717' },
  { name: 'Azure', value: '#0a6dd9' },
  { name: 'Forest', value: '#2d6a4f' },
  { name: 'Brick', value: '#b4402c' },
  { name: 'Ochre', value: '#a8761c' },
  { name: 'Plum', value: '#74407d' },
]

const KEY = 'plyback.appearance'
const DEFAULTS: Appearance = { mode: 'system', accent: DEFAULT_ACCENT, board: 'slate' }

function load(): Appearance {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '{}')
    const accent = typeof raw.accent === 'string' && /^#[0-9a-f]{6}$/i.test(raw.accent) ? raw.accent : DEFAULTS.accent
    return {
      mode: ['system', 'light', 'dark'].includes(raw.mode) ? raw.mode : DEFAULTS.mode,
      accent,
      board: ['slate', 'walnut', 'green', 'mono'].includes(raw.board) ? raw.board : DEFAULTS.board,
    }
  } catch {
    return DEFAULTS
  }
}

let state: Appearance = load()
const listeners = new Set<() => void>()

function luminance(hex: string): number {
  const n = parseInt(hex.slice(1), 16)
  return [n >> 16, (n >> 8) & 255, n & 255]
    .map(v => { const c = v / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4 })
    .reduce((acc, c, i) => acc + c * [0.2126, 0.7152, 0.0722][i], 0)
}

function contrast(a: number, b: number) {
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

/** Writes the cover colour and the derived, contrast-safe variants to CSS variables. */
export function applyAppearance(a: Appearance = state) {
  const root = document.documentElement
  const dark = a.mode === 'dark' || (a.mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches)
  root.dataset.theme = dark ? 'dark' : 'light'
  const L = luminance(a.accent)
  root.style.setProperty('--accent', a.accent)
  root.style.setProperty('--accent-fg', contrast(L, 1) >= contrast(L, 0.01) ? '#ffffff' : '#111111')
  // Text/lines in the accent colour must stay readable on the page background.
  const bgL = dark ? 0.004 : 0.955
  let ink = a.accent
  if (contrast(L, bgL) < 3.2) ink = `color-mix(in oklab, ${a.accent} ${dark ? 55 : 62}%, ${dark ? 'white' : 'black'})`
  root.style.setProperty('--accent-ink', ink)
  const icon = document.querySelector<HTMLLinkElement>('link[rel=icon]')
  if (icon) icon.href = `data:image/svg+xml,${encodeURIComponent(markSvg(a.accent === '#171717' ? '#8f8f8f' : a.accent))}`
}

export function setAppearance(patch: Partial<Appearance>) {
  state = { ...state, ...patch }
  try { localStorage.setItem(KEY, JSON.stringify(state)) } catch { /* storage unavailable */ }
  applyAppearance()
  listeners.forEach(l => l())
}

export function useAppearance(): Appearance {
  const a = useSyncExternalStore(cb => { listeners.add(cb); return () => listeners.delete(cb) }, () => state)
  useEffect(() => {
    if (a.mode !== 'system') return
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => applyAppearance()
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [a.mode])
  return a
}
