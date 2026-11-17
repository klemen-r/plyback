import { useSingleFlight } from '../lib/useSingleFlight'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { Mark } from '../components/Logo'
import { Button, ErrorNote, Input } from '../components/ui'
import { api } from '../lib/api'

export default function Login() {
  const [password, setPassword] = useState('')
  const qc = useQueryClient()
  const login = useMutation({
    mutationFn: () => api.login(password),
    onSuccess: s => { qc.setQueryData(['session'], s); qc.invalidateQueries() },
  })
  const signIn = useSingleFlight(login.mutateAsync)
  return (
    <div className="flex min-h-screen items-center justify-center bg-bg px-4">
      <form className="appear w-full max-w-[340px]" onSubmit={e => { e.preventDefault(); if (password) signIn() }}>
        <Mark size={32} />
        <h1 className="mt-6 text-[22px] font-semibold tracking-[-0.03em]">Sign in to Plyback</h1>
        <p className="mt-1 text-[14px] text-fg-2">This Plyback server is protected with a password.</p>
        <label className="mt-6 block text-[13px] font-medium" htmlFor="password">Password</label>
        <Input id="password" type="password" autoComplete="current-password" autoFocus value={password}
          onChange={e => setPassword(e.target.value)} className="mt-1.5 h-9 w-full" />
        {login.error && <div className="mt-3"><ErrorNote error={login.error} /></div>}
        <Button type="submit" variant="primary" className="mt-4 h-9 w-full" loading={login.isPending} disabled={!password}>Sign in</Button>
      </form>
    </div>
  )
}
