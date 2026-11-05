import { useQuery, useQueryClient } from '@tanstack/react-query'
import { lazy, Suspense, useEffect, useState } from 'react'
import { Route, Routes } from 'react-router-dom'
import Layout from './components/Layout'
import { Button, Empty, ErrorNote, Loading } from './components/ui'
import { api, authEvents } from './lib/api'
import { AccountProvider } from './lib/account'
import Login from './pages/Login'

const Overview = lazy(() => import('./pages/Overview'))
const Accounts = lazy(() => import('./pages/Accounts'))
const Games = lazy(() => import('./pages/Games'))
const GameReview = lazy(() => import('./pages/GameReview'))
const InsightsPage = lazy(() => import('./pages/Insights'))
const Practice = lazy(() => import('./pages/Practice'))
const Openings = lazy(() => import('./pages/Openings'))
const Compare = lazy(() => import('./pages/Compare'))
const Reports = lazy(() => import('./pages/Reports'))
const ReportView = lazy(() => import('./pages/ReportView'))
const ImportPgn = lazy(() => import('./pages/ImportPgn'))
const ActivityPage = lazy(() => import('./pages/Activity'))
const SettingsPage = lazy(() => import('./pages/Settings'))
const Methodology = lazy(() => import('./pages/Methodology'))

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
