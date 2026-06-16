// paavo-web — live "Recent jobs" updater for the dashboard (/).
//
// Loaded by the dashboard via `<script src="/static/dashboard-live.js?v=...">`.
// Opens an EventSource against `/api/dashboard/feed` and, on each
// `recent-jobs` event, swaps the table body and updates the row count.
// The payload is a JSON envelope `{count, tbody}` whose `tbody` is
// server-rendered, already-escaped HTML (see crates/paavo-web/src/feed.rs
// and pages/dashboard.rs::recent_jobs_tbody) — so assigning it via
// innerHTML introduces no new XSS surface.
//
// Vanilla DOM, no framework. No-op on any page lacking #recent-jobs-body.
(function () {
  'use strict';

  var body = document.getElementById('recent-jobs-body');
  if (!body) return; // not the dashboard
  var count = document.getElementById('recent-jobs-count');

  var es = new EventSource('/api/dashboard/feed');

  es.addEventListener('recent-jobs', function (e) {
    var d;
    try {
      d = JSON.parse(e.data);
    } catch (_err) {
      return; // ignore a malformed frame; the next push corrects it
    }
    if (typeof d.tbody === 'string') body.innerHTML = d.tbody;
    if (count && typeof d.count === 'number') count.textContent = d.count;
  });

  // EventSource auto-reconnects with backoff; the server re-sends the
  // current snapshot on connect, so there is nothing to recover here.
})();
