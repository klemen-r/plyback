import { SERIES } from '../lib/charts'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowRight, RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { LineChart } from '../components/charts'
import GameTable from '../components/GameTable'
import LatestGame from '../components/LatestGame'
import { Mark } from '../components/Logo'
import { Banner, Button, Card, Confidence, Empty, Figure, Figures, Loading, Meter, PageHeader, Progress, Section, Tabs } from '../components/ui'
import { useAccount } from '../lib/account-context'
import { api } from '../lib/api'
import { DAY, fmtDate, fmtDuration, nowTs, num, pct, timeAgo } from '../lib/format'
import { AddAccountForm } from './Accounts'
import { insightPollInterval } from '../lib/insight-query'

function Welcome() {
  return (
    <div className="appear mx-auto max-w-[620px] py-10 sm:py-16">
      <Mark size={36} />
      <h1 className="mt-8 text-[34px] font-semibold leading-tight tracking-[-0.04em] sm:text-[40px]">Every game you play, reviewed.</h1>
      <p className="mt-3 text-[16px] leading-relaxed text-fg-2">
        Connect a Chess.com username. Plyback imports your games, reviews every move with Stockfish on this
        computer, and shows the patterns that are costing you points — each one backed by positions from your own games.
      </p>
      <Card className="mt-8"><AddAccountForm /></Card>
      <dl className="mt-10 grid gap-6 text-[14px] sm:grid-cols-3">
        {[
          ['Automatic', 'New games are reviewed minutes after you finish them.'],
          ['Private', 'Runs locally. No sign-up, no subscription, no paid API.'],
          ['Evidence-based', 'Every finding links to the positions behind it.'],
        ].map(([t, d]) => <div key={t}><dt className="font-medium">{t}</dt><dd className="mt-1 text-fg-2">{d}</dd></div>)}
      </dl>
    </div>
  )
}

export default function Overview() {
  const { account, loading } = useAccount()
  const qc = useQueryClient()
  const [period, setPeriod] = useState('30')
  const since = period === 'all' ? undefined : Math.floor((nowTs() - Number(period) * DAY) / 3600) * 3600
  const recent = useQuery({
    queryKey: ['games', { account_id: account?.id, page_size: 10 }],
    queryFn: () => api.games({ account_id: account!.id, page_size: 10 }),
    enabled: !!account, refetchInterval: 15000,
  })
  const window_ = useQuery({ queryKey: ['insights', account?.id, { since }], queryFn: () => api.insights(account!.id, since ? { since } : {}), enabled: !!account, refetchInterval: insightPollInterval })
  const all = useQuery({ queryKey: ['insights', account?.id, {}], queryFn: () => api.insights(account!.id, {}), enabled: !!account, refetchInterval: insightPollInterval })
  const status = useQuery({ queryKey: ['status'], queryFn: api.status })
  const sync = useMutation({ mutationFn: () => api.syncAccount(account!.id), onSuccess: () => qc.invalidateQueries({ queryKey: ['accounts'] }) })

  if (loading) return <Loading />
  if (!account) return <Welcome />

  const d = window_.data
  const ratings = all.data ? Object.entries(all.data.ratings).sort((a, b) => b[1].games - a[1].games) : []
  const [mainTc, mainRating] = ratings[0] ?? []
  const q = status.data?.queue
  const priorities = (all.data?.priorities ?? []).filter(p => p.rank).slice(0, 4)
  const maxLoss = Math.max(0.01, ...priorities.map(p => p.ep_lost_per_game ?? 0))
  const ratingLine = Object.entries(account.ratings).filter(([, r]) => r.rating).map(([tc, r]) => `${tc[0].toUpperCase()}${tc.slice(1)} ${r.rating}`).join(' · ')

  return (
    <div className="appear space-y-7">
      <PageHeader title="Overview"
        description={<>{ratingLine || 'No ratings yet'} <span className="text-fg-3">· synced {timeAgo(account.last_sync_at)}</span></>}
        actions={<Button onClick={() => sync.mutate()} loading={sync.isPending || account.sync_status === 'syncing'}><RefreshCw className="h-3.5 w-3.5" />Sync now</Button>} />

      {account.sync_status === 'error' && <Banner tone="warn">Last sync failed: {account.sync_error} It will retry automatically.</Banner>}
      {(account.queued > 0 || (q && q.running > 0)) && (
        <Banner actions={<Link to="/activity" className="text-[13px] text-fg-2 hover:text-fg">Details</Link>}>
          <div className="flex items-center justify-between gap-4">
            <span>Reviewing games with {status.data?.engine.name ?? 'Stockfish'}</span>
            <span className="tnum text-fg-3">{account.analyzed} of {account.games}{q?.eta_seconds ? ` · about ${fmtDuration(q.eta_seconds)} left` : ''}</span>
          </div>
          <Progress value={account.analyzed} max={account.games} className="mt-2.5" />
        </Banner>
      )}

      <div>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <Tabs label="Period" value={period} onChange={setPeriod}
            options={[{ value: '1', label: '24 hours' }, { value: '7', label: '7 days' }, { value: '30', label: '30 days' }, { value: '90', label: '90 days' }, { value: 'all', label: 'All time' }]} />
          {d && <Confidence c={d.data_quality.confidence} n={d.data_quality.analyzed} />}
        </div>
        {d ? (
          <Figures cols={5}>
            <Figure label="Games" value={d.overview.games} detail={`${d.overview.wins} won · ${d.overview.draws} drawn · ${d.overview.losses} lost`} />
            <Figure label="Score" value={pct(d.overview.score)} detail={`White ${pct(d.overview.white.score)} · Black ${pct(d.overview.black.score)}`} />
            <Figure label="Accuracy" value={num(d.overview.accuracy, 1)} detail={`Opponents ${num(d.overview.opp_accuracy, 1)}`}
              hint="Stockfish-based accuracy (Lichess formula). Not the same number as Chess.com's." />
            <Figure label="Blunders per game" value={num(d.overview.per_game?.blunder, 2)} detail={`Opponents ${num(d.overview.opp_per_game?.blunder, 2)}`} />
            {mainTc && d.ratings[mainTc]
              ? <Figure label={`${mainTc[0].toUpperCase()}${mainTc.slice(1)} rating`} value={d.ratings[mainTc].end}
                  detail={<span className={d.ratings[mainTc].change >= 0 ? 'text-good' : 'text-bad'}>{d.ratings[mainTc].change >= 0 ? '+' : ''}{d.ratings[mainTc].change} in this period</span>} />
              : <Figure label="Rating" value="—" />}
          </Figures>
        ) : <div className="skeleton h-[98px]" />}
      </div>

      {/* Phones: latest game, recent games, priorities. Wide screens: recent games beside the other two. */}
      <div className="grid gap-7 lg:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)] lg:grid-rows-[auto_1fr] lg:gap-x-6 lg:[grid-template-areas:'recent_latest'_'recent_improve']">
        {recent.data?.items[0] && (
          <Section title="Latest game" className="lg:[grid-area:latest]">
            <Card padded={false}><LatestGame g={recent.data.items[0]} /></Card>
          </Section>
        )}
        <Section className="lg:[grid-area:recent]" title="Recent games" actions={<Link to="/games" className="flex items-center gap-1 text-fg-2 hover:text-fg">All games <ArrowRight className="h-3.5 w-3.5" /></Link>}>
          <Card padded={false} className="overflow-hidden">
            {recent.data?.items.length ? <GameTable games={recent.data.items} compact />
              : recent.isLoading ? <Loading /> : <Empty title="No games yet">They appear here as soon as the first sync finishes.</Empty>}
          </Card>
        </Section>
        <Section className="lg:[grid-area:improve]" title="Where to improve" actions={<Link to="/insights" className="flex items-center gap-1 text-fg-2 hover:text-fg">Insights <ArrowRight className="h-3.5 w-3.5" /></Link>}>
          <Card padded={false}>
            {priorities.length ? priorities.map((p, i) => (
              <Link key={p.key} to="/insights#priorities" className={'block px-4 py-3.5 transition-colors hover:bg-hover/60' + (i ? ' border-t border-line' : '')}>
                <div className="flex items-baseline justify-between gap-3">
                  <span className="font-medium">{p.title}</span>
                  <span className="tnum shrink-0 text-[13px] text-fg-2">{((p.ep_lost_per_game ?? 0) * 100).toFixed(0)}% / game</span>
                </div>
                <div className="mt-0.5 text-[13px] text-fg-2">{p.errors_per_game?.toFixed(2)} per game · opponents {p.opp_errors_per_game?.toFixed(2)}</div>
                <div className="mt-2.5"><Meter value={p.ep_lost_per_game ?? 0} max={maxLoss} tone={i === 0 ? 'accent' : 'fg'} /></div>
              </Link>
            )) : <Empty title="Not enough reviewed games yet">Priorities appear once enough games are analysed to separate patterns from noise.</Empty>}
          </Card>
        </Section>
      </div>

      {mainRating && (
        <Section title={`${mainTc![0].toUpperCase()}${mainTc!.slice(1)} rating`} description={`${mainRating.games} rated games · peak ${mainRating.peak}`}>
          <Card>
            <LineChart data={mainRating.points} x="t" height={200} xFormat={v => fmtDate(Number(v), false, true)} series={[{ key: 'rating', name: 'Rating', color: SERIES.you }]} />
          </Card>
        </Section>
      )}
    </div>
  )
}
