import { platformName } from '../lib/platform'

/** Compact, text-based source marks; also distinguish matching account names. */
export default function PlatformMark({ platform }: { platform: string }) {
  if (platform !== 'chesscom' && platform !== 'lichess') return null
  return <span role="img" aria-label={platformName(platform)} title={platformName(platform)}
    className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-[2px] border border-line-strong text-[10px] font-semibold leading-none text-fg-2">{platform === 'lichess' ? 'L' : 'C'}</span>
}
