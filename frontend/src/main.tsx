import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import App from './App'
import { applyAppearance } from './lib/theme'
import './index.css'

applyAppearance()

// After an upgrade, a tab opened on the previous build asks for page chunks that no longer exist.
// Reload to pick up the new build, but never twice within a few seconds, so a chunk that is
// genuinely missing cannot cause a reload loop.
window.addEventListener('vite:preloadError', event => {
  const key = 'plyback.chunk-reload'
  let last: number
  try {
    last = Number(sessionStorage.getItem(key)) || 0
    sessionStorage.setItem(key, String(Date.now()))
  } catch { return }
  if (Date.now() - last < 10_000) return
  event.preventDefault()
  location.reload()
})

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 5_000,
      refetchOnWindowFocus: false,
      retry: (count, err) => count < 2 && !(err as { status?: number }).status,
    },
  },
})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
)
