import clsx from 'clsx'
import { TIME_CLASSES, type FilterState } from '../lib/filters'
import { Input, Select, Tabs } from './ui'

export default function FilterBar({ value, onChange, openings }:
  { value: FilterState; onChange: (v: FilterState) => void; openings?: { family: string; n: number }[] }) {
  const set = (patch: Partial<FilterState>) => onChange({ ...value, ...patch })
  const toggleTc = (tc: string) => set({ time_classes: value.time_classes.includes(tc) ? value.time_classes.filter(t => t !== tc) : [...value.time_classes, tc] })
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Tabs label="Period" value={value.period} onChange={v => set({ period: v })}
        options={[{ value: '30', label: '30d' }, { value: '90', label: '90d' }, { value: '365', label: '1y' }, { value: 'all', label: 'All' }, { value: 'custom', label: 'Custom' }]} />
      {value.period === 'custom' && (
        <span className="flex items-center gap-1.5">
          <Input type="date" aria-label="From" value={value.from ?? ''} onChange={e => set({ from: e.target.value })} />
          <span className="text-fg-3">–</span>
          <Input type="date" aria-label="To" value={value.to ?? ''} onChange={e => set({ to: e.target.value })} />
        </span>
      )}
      <div className="inline-flex h-8 items-center overflow-hidden rounded-md border border-line bg-surface" role="group" aria-label="Time control">
        {TIME_CLASSES.map((tc, i) => (
          <button key={tc} type="button" aria-pressed={value.time_classes.includes(tc)} onClick={() => toggleTc(tc)}
            className={clsx('h-full px-2.5 text-[13px] capitalize transition-colors', i > 0 && 'border-l border-line',
              value.time_classes.includes(tc) ? 'bg-fg text-bg' : 'text-fg-2 hover:bg-hover hover:text-fg')}>{tc}</button>
        ))}
      </div>
      <Select label="Colour" value={value.color} onChange={v => set({ color: v as FilterState['color'] })}>
        <option value="">Both colours</option><option value="white">White</option><option value="black">Black</option>
      </Select>
      <Select label="Rated" value={value.rated} onChange={v => set({ rated: v as FilterState['rated'] })}>
        <option value="">Rated and casual</option><option value="true">Rated</option><option value="false">Casual</option>
      </Select>
      {openings && openings.length > 0 && (
        <Select label="Opening" value={value.opening} onChange={v => set({ opening: v })} className="max-w-[220px]">
          <option value="">All openings</option>
          {openings.map(o => <option key={o.family} value={o.family}>{o.family} ({o.n})</option>)}
        </Select>
      )}
    </div>
  )
}
