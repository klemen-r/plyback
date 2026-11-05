// Unchanged insights are refreshed by the database revision in AccountProvider.
// Keep polling stale server-cache results and retry transient failures even when
// no new games arrive (window-focus refetching is disabled).
export function insightPollInterval(query: { state: { error: unknown; data?: { stale?: boolean } } }) {
  return query.state.error ? 15000 : query.state.data?.stale ? 2000 : false
}
