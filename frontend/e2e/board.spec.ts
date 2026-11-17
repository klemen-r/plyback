import { expect, test, type Page } from '@playwright/test'
import type { PracticeItem, PracticeResult } from '../src/lib/types'
import { account, mockApi, playMove } from './support'

async function promotionPuzzle(page: Page) {
  const stats = { total: 1, new: 1, due: 0, learning: 0, mastered: 0, next_due_at: null }
  const item: PracticeItem = {
    game_id: 42, ply: 1, fen: '7k/P7/8/8/8/8/8/7K w - - 0 42', color: 'white', move_number: 42,
    classification: 'mistake', phase: 'endgame', ep_loss: 0.2, last_uci: null, last_san: null,
    end_time: null, time_class: 'rapid', opponent: 'Fixture', opponent_rating: null,
    themes: ['promotion'], status: 'new', box: 0, attempts: 0, correct: 0,
  }
  const answer: PracticeResult = {
    correct: false, uci: null, san: null, best_uci: 'a7a8q', best_san: 'a8=Q+', line_san: ['a8=Q+'],
    game_san: 'Kh2', explanation: null, box: 0, due_at: 9999999999, stats, themes: ['promotion'],
  }
  await mockApi(page, async route => {
    await route.fulfill({ json: route.request().method() === 'POST' ? answer : { stats, items: [item] } })
  }, undefined, { accounts: [account({ games: 1, analyzed: 1 })] })
  await page.goto('/practice')
  await playMove(page, 'a7', 'a8')
  await expect(page.getByRole('group', { name: 'Promote pawn' })).toBeVisible()
}

test('revealing a practice answer closes a pending promotion', async ({ page }) => {
  await promotionPuzzle(page)
  await page.getByRole('button', { name: 'Show answer', exact: true }).click()
  await expect(page.getByText('The move was a8=Q+', { exact: true })).toBeVisible()
  await expect(page.getByRole('group', { name: 'Promote pawn' })).toHaveCount(0)
})

test('a pending promotion blocks typed practice moves until cancelled', async ({ page }) => {
  await promotionPuzzle(page)
  await expect(page.getByLabel('Type your move')).toBeDisabled()
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeDisabled()
  await page.keyboard.press('Escape')
  await expect(page.getByLabel('Type your move')).toBeEnabled()
  await page.getByLabel('Type your move').fill('a8=Q+')
  await expect(page.getByRole('button', { name: 'Play', exact: true })).toBeEnabled()
})
