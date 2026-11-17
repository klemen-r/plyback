export function platformName(platform: string): string {
  return platform === 'lichess' ? 'Lichess' : platform === 'chesscom' ? 'Chess.com' : 'Original game'
}

/** The site a game's link opens, for "View on …": the platform, else the link's host. */
export function linkedSite(source: string, url: string): string {
  if (source === 'lichess' || source === 'chesscom') return platformName(source)
  try {
    const host = new URL(url).hostname.replace(/^www\./, '')
    return host === 'lichess.org' ? 'Lichess' : host === 'chess.com' ? 'Chess.com' : host
  } catch {
    return 'the original site'
  }
}
