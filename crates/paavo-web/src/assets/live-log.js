// paavo-web — live log + phase banner consumer for /jobs/:id.
//
// Loaded by the job-detail page via `<script src="/static/live-log.js?v=...">`.
// Pulls the job id from the `<pre id="logpane">` element's
// `data-job-id` attribute, opens an EventSource against
// `/api/jobs/:id/stream`, and dispatches the named SSE events
// emitted by paavo-web's SSE proxy (see `crates/paavo-web/src/proxy.rs`):
//
//   event: frame      → append a coloured line to the log pane
//   event: phase      → update the phase banner
//   event: lagged     → append a "lagged" line to the log pane
//   event: terminal   → render the outcome card; stop the stream
//   event: truncated  → append a "truncated" line; stop the stream
//
// The proxy already enriches `frame` events with `display_ts` (server-
// side `mm:ss.fff` format) and `phase` (current phase from the proxy
// cursor), so this consumer is stateless beyond "did we see a
// terminal?".
//
// Vanilla DOM, no framework. Runs in every modern browser back to
// Edge 79; EventSource has been stable since Firefox 6 / Chrome 6.
(function () {
  'use strict';

  const pane = document.getElementById('logpane');
  if (!pane) return; // Not the job-detail page; nothing to do.
  const jobId = pane.dataset.jobId;
  if (!jobId) {
    console.warn('paavo-web live-log: no data-job-id on #logpane; aborting');
    return;
  }
  const banner = document.getElementById('phase-banner');
  const status = document.getElementById('stream-status');
  const outcomeBox = document.getElementById('outcome-card');
  const outcomePre = document.getElementById('outcome-json');

  // Track terminal state so onerror's auto-reconnect doesn't keep
  // hammering the server after the stream has closed cleanly.
  let closed = false;

  // Highest seq already rendered. Initialized from the SSR-emitted
  // data-since-seq (the last frame baked into the page). Every frame
  // with seq <= lastSeq is dropped, making the consumer idempotent
  // under historical replay, the broadcast-buffer race, and reconnects.
  let lastSeq = parseInt(pane.dataset.sinceSeq || '-1', 10);
  if (Number.isNaN(lastSeq)) lastSeq = -1;

  // Pass since_seq upstream so the proxy doesn't re-ship the SSR
  // prefix on the initial connect. Only meaningful when the page
  // baked in historical frames.
  const sinceQuery =
    pane.dataset.sinceSeq != null
      ? '?since_seq=' + encodeURIComponent(pane.dataset.sinceSeq)
      : '';
  const es = new EventSource(
    '/api/jobs/' + encodeURIComponent(jobId) + '/stream' + sinceQuery
  );

  // Append a `<span class="log-line ...">` to the pane and
  // auto-scroll if the operator is already pinned to the bottom.
  // Manual scrollback is preserved (we don't yank them back).
  function appendLine(html, classes) {
    const atBottom = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 32;
    const span = document.createElement('span');
    span.className = 'log-line ' + (classes || '');
    span.innerHTML = html;
    pane.appendChild(span);
    if (atBottom) {
      pane.scrollTop = pane.scrollHeight;
    }
  }

  // Escape user-controlled message bodies so a defmt frame
  // containing `<script>` doesn't escape the pane.
  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#x27;');
  }

  function setBanner(phase) {
    if (!banner) return;
    banner.textContent = 'phase: ' + phase;
    banner.className = 'phase-banner ' + phase;
  }

  function setStatus(text) {
    if (status) status.textContent = text;
  }

  es.addEventListener('frame', function (e) {
    let f;
    try {
      f = JSON.parse(e.data);
    } catch (_err) {
      console.warn('paavo-web live-log: bad frame JSON', e.data);
      return;
    }
    // Idempotency: drop any frame we've already rendered. Closes the
    // historical-replay, broadcast-buffer-race, and reconnect dup
    // sources with one mechanism (see the C2 spec §5).
    if (typeof f.seq === 'number') {
      if (f.seq <= lastSeq) return;
      lastSeq = f.seq;
    }
    // Phase tag: prefer the proxy's enrichment; fall back to an EXACT
    // inference from target (cargo:* => build, else => run) so
    // stream-replayed historical frames — which carry no Phase events —
    // tag identically to SSR-rendered ones.
    const phase =
      f.phase || (f.target && f.target.indexOf('cargo:') === 0 ? 'build' : 'run');
    const phaseClass = 'phase-' + phase;
    const lvlClass = 'lvl-' + (f.level || 'info');
    const tag = '[' + phase + ']\u00a0';
    const ts = f.display_ts || '';
    const lvl = (f.level || 'info').toUpperCase();
    const html =
      '<span class="' + phaseClass + '">' + tag + '</span>' +
      '<span class="log-ts">' + escapeHtml(ts) + '</span> ' +
      '[' + lvl + '] ' +
      escapeHtml(f.message || '') +
      '\n';
    appendLine(html, lvlClass);
  });

  es.addEventListener('phase', function (e) {
    try {
      const p = JSON.parse(e.data);
      if (p && typeof p.phase === 'string') setBanner(p.phase);
    } catch (_err) {
      console.warn('paavo-web live-log: bad phase JSON', e.data);
    }
  });

  es.addEventListener('lagged', function (e) {
    let missed = 0;
    try {
      missed = JSON.parse(e.data).missed | 0;
    } catch (_err) {
      // pass
    }
    appendLine(
      '── lagged: ' + missed + ' frame(s) missed; refresh for the full log ──\n',
      'lvl-warn'
    );
  });

  es.addEventListener('terminal', function (e) {
    let outcome = null;
    try {
      outcome = JSON.parse(e.data).outcome;
    } catch (_err) {
      // pass
    }
    if (outcomeBox && outcomePre) {
      outcomePre.textContent = JSON.stringify(outcome, null, 2);
      outcomeBox.hidden = false;
    }
    setBanner('done');
    setStatus('stream closed (terminal)');
    closed = true;
    es.close();
  });

  es.addEventListener('truncated', function (e) {
    let reason = '';
    try {
      reason = JSON.parse(e.data).reason || '';
    } catch (_err) {
      // pass
    }
    appendLine(
      '── truncated: ' + escapeHtml(reason) + ' ──\n',
      'lvl-error'
    );
    setBanner('done');
    setStatus('stream closed (truncated)');
    closed = true;
    es.close();
  });

  es.onerror = function () {
    if (closed) return; // Don't fight the auto-reconnect after a clean close.
    setStatus('reconnecting…');
    // EventSource auto-reconnects with exponential backoff; let it.
  };

  es.onopen = function () {
    if (closed) return;
    setStatus('streaming live');
  };
})();
