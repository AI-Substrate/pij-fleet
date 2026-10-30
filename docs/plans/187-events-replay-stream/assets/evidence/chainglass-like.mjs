// Plan 187: a /v1/events client that behaves like chainglass's
// rs-event-stream.ts + rs-client.ts (read 2026-10-30, not imported):
//   - recycles the connection every RECYCLE_MS (5 s);
//   - starts a fresh process at cursor 0 for each machine alias;
//   - advances its resume cursor ONLY on the five RESUME_EVENT_KINDS, in wire order;
//   - parses NDJSON the same way (TextDecoder, split on '\n', JSON.parse per line).
// usage: node chainglass-like.mjs <port> <key> <alias> <seconds> [startCursor]
// Prints one JSON line per connection and a summary line at the end.
const [port, key, alias, seconds, start] = process.argv.slice(2);
const RECYCLE_MS = 5_000;
const RESUME_EVENT_KINDS = new Set(['seat.put', 'seat.tombstone', 'role-set', 'report.now', 'report.state']);
const resume = { [alias]: Number(start ?? 0) };
const deadline = Date.now() + Number(seconds) * 1000;
let connections = 0;
let totalFrames = 0;
const runStart = Date.now();

while (Date.now() < deadline) {
  connections += 1;
  const before = resume[alias];
  const t0 = performance.now();
  let helloMs;
  let firstEventMs;
  let frames = 0;
  let lastCursor = before;
  let error;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), RECYCLE_MS);
  try {
    const url = `http://127.0.0.1:${port}/v1/events?since=${encodeURIComponent(JSON.stringify(resume))}`;
    const response = await fetch(url, {
      headers: { authorization: `Bearer ${key}`, accept: 'application/x-ndjson' },
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`http ${response.status}: ${await response.text()}`);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let pending = '';
    while (true) {
      const { done, value } = await reader.read();
      pending += decoder.decode(value, { stream: !done });
      let newline = pending.indexOf('\n');
      while (newline !== -1) {
        const line = pending.slice(0, newline).trim();
        pending = pending.slice(newline + 1);
        if (line) {
          const frame = JSON.parse(line);
          if (frame.hello === true) {
            helloMs ??= performance.now() - t0;
          } else if (frame.type === 'event') {
            firstEventMs ??= performance.now() - t0;
            frames += 1;
            lastCursor = frame.cursor;
            if (RESUME_EVENT_KINDS.has(frame.event.kind)) {
              resume[frame.machine] = Math.max(resume[frame.machine] ?? 0, frame.cursor);
            }
          }
        }
        newline = pending.indexOf('\n');
      }
      if (done) break;
    }
  } catch (cause) {
    if (cause.name !== 'AbortError') error = String(cause);
  } finally {
    clearTimeout(timer);
  }
  totalFrames += frames;
  console.log(
    JSON.stringify({
      conn: connections,
      at_s: Math.round((Date.now() - runStart) / 100) / 10,
      cursor_before: before,
      cursor_after: resume[alias],
      hello_ms: helloMs === undefined ? null : Math.round(helloMs),
      first_event_ms: firstEventMs === undefined ? null : Math.round(firstEventMs),
      frames,
      last_frame_cursor: lastCursor,
      error,
    }),
  );
  if (error) await new Promise((r) => setTimeout(r, 250));
}
console.log(JSON.stringify({ summary: true, connections, total_frames: totalFrames, final_cursor: resume[alias] }));
