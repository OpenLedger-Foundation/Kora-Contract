export function exportProtocolEventsAsJson(events: unknown[]) {
  return JSON.stringify(events, null, 2);
}

export function exportProtocolEventsAsCsv(rows: Array<Record<string, string | number>>) {
  if (rows.length === 0) return "";
  const headers = Object.keys(rows[0]);
  return [headers.join(","), ...rows.map((row) => headers.map((header) => row[header]).join(","))].join("\n");
}
