import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Suspense, useEffect, useState } from 'react'
import { Route, Routes } from 'react-router-dom'
import Layout from './components/Layout'
import { Button, Empty, ErrorNote, Loading } from './components/ui'
import { api, authEvents } from './lib/api'
import { AccountProvider } from './lib/account'
import { lazyPage } from './lib/lazy-page'
import Login from './pages/Login'

const Overview = lazyPage(() => import('./pages/Overview'))
const Accounts = lazyPage(() => import('./pages/Accounts'))
const Games = lazyPage(() => import('./pages/Games'))
const GameReview = lazyPage(() => import('./pages/GameReview'))
const InsightsPage = lazyPage(() => import('./pages/Insights'))
const Practice = lazyPage(() => import('./pages/Practice'))
const Openings = lazyPage(() => import('./pages/Openings'))
const Compare = lazyPage(() => import('./pages/Compare'))
const Reports = lazyPage(() => import('./pages/Reports'))
const ReportView = lazyPage(() => import('./pages/ReportView'))
const ImportPgn = lazyPage(() => import('./pages/ImportPgn'))
const ActivityPage = lazyPage(() => import('./pages/Activity'))
const SettingsPage = lazyPage(() => import('./pages/Settings'))
const Methodology = lazyPage(() => import('./pages/Methodology'))

export default function App() {
  const qc = useQueryClient()
  const [logoutError, setLogoutError] = useState<unknown>(null)
  const session = useQuery({ queryKey: ['session'], queryFn: api.session, staleTime: Infinity })
  useEffect(() => {
    const onUnauthorized = () => qc.invalidateQueries({ queryKey: ['session'] })
    authEvents.addEventListener('unauthorized', onUnauthorized)
    return () => authEvents.removeEventListener('unauthorized', onUnauthorized)
  }, [qc])

  if (session.isLoading) return <Loading />
  if (session.error) return <div className="mx-auto max-w-lg p-8"><ErrorNote error={session.error} /><Button className="mt-4" onClick={() => session.refetch()}>Retry connection</Button></div>
  if (session.data && !session.data.authenticated) return <Login />
  const logout = session.data?.auth_required
    ? async () => {
      try {
        await api.logout()
        await qc.cancelQueries()
        qc.removeQueries({ predicate: query => query.queryKey[0] !== 'session' })
        qc.setQueryData(['session'], { authenticated: false, password_set: true, auth_required: true })
        setLogoutError(null)
      } catch (error) { setLogoutError(error) }
    }
    : undefined

  return (
    <AccountProvider><Layout onLogout={logout}>
      {!!logoutError && <ErrorNote error={logoutError} />}
      <Suspense fallback={<Loading />}>
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/accounts" element={<Accounts />} />
          <Route path="/games" element={<Games />} />
          <Route path="/games/:id" element={<GameReview />} />
          <Route path="/insights" element={<InsightsPage />} />
          <Route path="/practice" element={<Practice />} />
          <Route path="/openings" element={<Openings />} />
          <Route path="/compare" element={<Compare />} />
          <Route path="/reports" element={<Reports />} />
          <Route path="/reports/:id" element={<ReportView />} />
          <Route path="/import" element={<ImportPgn />} />
          <Route path="/activity" element={<ActivityPage />} />
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/methodology" element={<Methodology />} />
          <Route path="*" element={<Empty title="Page not found">The page you were looking for doesn’t exist.</Empty>} />
        </Routes>
      </Suspense>
    </Layout></AccountProvider>
  )
}
