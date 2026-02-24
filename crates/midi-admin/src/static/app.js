/* ═══════════════════════════════════════════════════════════
   MIDInet Dashboard — Real-time monitoring
   Vanilla JS · WebSocket · Canvas sparkline
   ═══════════════════════════════════════════════════════════ */
(function () {
    'use strict';

    // ── DOM shortcuts ──
    const $ = (id) => document.getElementById(id);

    // ── State ──
    const S = {
        ws: null,
        reconnectMs: 1000,
        maxReconnectMs: 30000,
        connected: false,
        memTotal: 1024,
        // Sparkline ring buffer — 120 samples (2 min at 1/s)
        spark: new Float32Array(120),
        sparkLen: 0,
        sparkIdx: 0,
    };

    // ── Bootstrap ──
    document.addEventListener('DOMContentLoaded', () => {
        connectWs();
        bindEvents();
        resizeCanvas();
        fetchHosts();
        fetchClients();
        fetchAlerts();
        fetchSystemMetrics();
        window.addEventListener('resize', resizeCanvas);
    });

    // ────────────────────────────────────────────────────────
    //  WebSocket
    // ────────────────────────────────────────────────────────
    function connectWs() {
        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        S.ws = new WebSocket(proto + '//' + location.host + '/ws/status');

        S.ws.onopen = () => {
            S.connected = true;
            S.reconnectMs = 1000;
            setConn(true);
        };

        S.ws.onmessage = (ev) => {
            try { onStatus(JSON.parse(ev.data)); }
            catch (e) { console.error('ws parse', e); }
        };

        S.ws.onclose = () => {
            S.connected = false;
            setConn(false);
            setTimeout(connectWs, S.reconnectMs);
            S.reconnectMs = Math.min(S.reconnectMs * 2, S.maxReconnectMs);
        };

        S.ws.onerror = () => S.ws.close();
    }

    function setConn(ok) {
        $('connDot').className = 'conn-dot ' + (ok ? 'ok' : 'err');
        $('connText').textContent = ok ? 'Connected' : 'Reconnecting';
    }

    // ────────────────────────────────────────────────────────
    //  Status update (from WebSocket every 1 s)
    // ────────────────────────────────────────────────────────
    function onStatus(d) {
        // KPI cards
        setKpi('healthScore', d.health_score);
        setKpi('midiRate', Math.round(d.midi?.messages_per_sec ?? 0));
        setKpi('clientCount', d.client_count ?? 0);
        setKpi('alertCount', d.active_alerts ?? 0);

        // Health level
        const hs = d.health_score ?? 100;
        $('kpiHealth').dataset.level = hs >= 80 ? 'ok' : hs >= 50 ? 'warn' : 'crit';
        $('kpiAlerts').dataset.level = (d.active_alerts ?? 0) > 0 ? 'crit' : 'ok';

        // MIDI stats
        $('midiIn').textContent = Math.round(d.midi?.messages_per_sec ?? 0);
        $('midiOut').textContent = Math.round(d.midi?.messages_per_sec ?? 0);
        $('activeNotes').textContent = d.midi?.active_notes ?? 0;

        // Sparkline
        pushSpark(d.midi?.messages_per_sec ?? 0);
        drawSpark();

        // Failover
        const ah = d.failover?.active_host ?? 'primary';
        $('foHost').textContent = ah;
        $('activeHostPill').textContent = capitalize(ah);
        $('standbyDot').className = 'status-dot ' + (d.failover?.standby_healthy ? 'ok' : 'warn');

        // System resources
        updateMeters(d);

        // Uptime
        $('uptimeVal').textContent = fmtUptime(d.uptime ?? 0);
    }

    // ────────────────────────────────────────────────────────
    //  KPI animated counter
    // ────────────────────────────────────────────────────────
    function setKpi(id, val) {
        const el = $(id);
        if (el._val === val) return;
        el._val = val;
        el.textContent = typeof val === 'number' ? val.toLocaleString() : val;
        el.classList.remove('pulse');
        void el.offsetWidth; // reflow to retrigger animation
        el.classList.add('pulse');
    }

    // ────────────────────────────────────────────────────────
    //  Meters
    // ────────────────────────────────────────────────────────
    function updateMeters(d) {
        const cpu = d.cpu_percent ?? 0;
        setMeter('cpuFill', cpu, 100, false);
        $('cpuVal').textContent = cpu.toFixed(0) + '%';

        const memUsed = d.memory_used_mb ?? 0;
        const memPct = S.memTotal > 0 ? (memUsed / S.memTotal) * 100 : 0;
        setMeter('memFill', memPct, 100, false);
        $('memVal').textContent = memUsed + ' MB';

        const temp = d.cpu_temp_c ?? 0;
        setMeter('tempFill', temp, 100, true);
        $('tempVal').textContent = temp.toFixed(0) + ' \u00B0C';
    }

    function setMeter(id, val, max, isTemp) {
        const el = $(id);
        const pct = Math.min((val / max) * 100, 100);
        el.style.width = pct + '%';
        el.classList.remove('hot', 'crit');
        if (isTemp) {
            if (val >= 80) el.classList.add('crit');
            else if (val >= 65) el.classList.add('hot');
        } else {
            if (pct >= 90) el.classList.add('crit');
            else if (pct >= 75) el.classList.add('hot');
        }
    }

    // ────────────────────────────────────────────────────────
    //  Sparkline (canvas)
    // ────────────────────────────────────────────────────────
    function resizeCanvas() {
        const c = $('sparkCanvas');
        const rect = c.parentElement.getBoundingClientRect();
        c.width = rect.width * devicePixelRatio;
        c.height = 100 * devicePixelRatio;
        c.style.height = '100px';
        drawSpark();
    }

    function pushSpark(v) {
        S.spark[S.sparkIdx] = v;
        S.sparkIdx = (S.sparkIdx + 1) % S.spark.length;
        if (S.sparkLen < S.spark.length) S.sparkLen++;
    }

    function drawSpark() {
        const c = $('sparkCanvas');
        const ctx = c.getContext('2d');
        const W = c.width, H = c.height;
        const n = S.sparkLen;
        if (n < 2) { ctx.clearRect(0, 0, W, H); return; }

        // Gather values oldest to newest
        const vals = [];
        const start = (S.sparkIdx - n + S.spark.length) % S.spark.length;
        for (let i = 0; i < n; i++) vals.push(S.spark[(start + i) % S.spark.length]);

        const peak = Math.max(...vals, 1);
        const pad = 4 * devicePixelRatio;
        const drawH = H - pad * 2;
        const stepX = (W - pad * 2) / (S.spark.length - 1);

        ctx.clearRect(0, 0, W, H);

        // Gradient fill
        const grad = ctx.createLinearGradient(0, pad, 0, H);
        grad.addColorStop(0, 'rgba(191,90,242,.35)');
        grad.addColorStop(1, 'rgba(191,90,242,.02)');

        // Build path
        ctx.beginPath();
        const offsetX = (S.spark.length - n) * stepX;
        for (let i = 0; i < n; i++) {
            const x = pad + offsetX + i * stepX;
            const y = pad + drawH - (vals[i] / peak) * drawH;
            if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
        }

        // Stroke line
        ctx.strokeStyle = 'rgba(191,90,242,.9)';
        ctx.lineWidth = 1.5 * devicePixelRatio;
        ctx.lineJoin = 'round';
        ctx.stroke();

        // Fill area under curve
        const lastX = pad + offsetX + (n - 1) * stepX;
        const firstX = pad + offsetX;
        ctx.lineTo(lastX, H);
        ctx.lineTo(firstX, H);
        ctx.closePath();
        ctx.fillStyle = grad;
        ctx.fill();

        // Current value dot
        if (n > 0) {
            const cx = lastX;
            const cy = pad + drawH - (vals[n - 1] / peak) * drawH;
            ctx.beginPath();
            ctx.arc(cx, cy, 3 * devicePixelRatio, 0, Math.PI * 2);
            ctx.fillStyle = '#bf5af2';
            ctx.fill();
        }
    }

    // ────────────────────────────────────────────────────────
    //  REST fetches (initial + periodic)
    // ────────────────────────────────────────────────────────
    function fetchHosts() {
        fetch('/api/hosts')
            .then(r => r.json())
            .then(d => renderHosts(d.hosts || []))
            .catch(() => {});
        setTimeout(fetchHosts, 5000);
    }

    function fetchClients() {
        fetch('/api/clients')
            .then(r => r.json())
            .then(d => renderClients(d.clients || []))
            .catch(() => {});
        setTimeout(fetchClients, 5000);
    }

    function fetchAlerts() {
        fetch('/api/alerts')
            .then(r => r.json())
            .then(d => renderAlerts(d.alerts || []))
            .catch(() => {});
        setTimeout(fetchAlerts, 10000);
    }

    function fetchSystemMetrics() {
        fetch('/api/metrics/system')
            .then(r => r.json())
            .then(d => { S.memTotal = d.memory_total_mb || 1024; })
            .catch(() => {});
        setTimeout(fetchSystemMetrics, 30000);
    }

    // ────────────────────────────────────────────────────────
    //  Safe DOM renderers (no innerHTML)
    // ────────────────────────────────────────────────────────
    function renderHosts(hosts) {
        const container = $('hostList');
        container.replaceChildren();
        if (!hosts.length) {
            container.appendChild(makePlaceholder('Waiting for hosts\u2026'));
            return;
        }
        hosts.forEach(h => {
            const row = el('div', 'host-row');
            row.appendChild(el('span', 'host-dot ' + (h.role === 'primary' ? 'active' : 'standby')));
            row.appendChild(elText('span', h.name, 'host-name'));
            row.appendChild(elText('span', h.role, 'host-role'));
            row.appendChild(elText('span', h.ip, 'host-ip'));
            row.appendChild(elText('span', h.last_heartbeat_ms + 'ms', 'host-hb'));
            container.appendChild(row);
        });
    }

    function renderClients(clients) {
        const container = $('clientList');
        container.replaceChildren();
        if (!clients.length) {
            container.appendChild(makePlaceholder('No clients connected'));
            return;
        }
        clients.forEach(c => {
            const row = el('div', 'client-row');
            row.appendChild(el('span', 'client-dot'));
            row.appendChild(elText('span', c.hostname || c.ip, 'client-name'));
            row.appendChild(elText('span', c.ip, 'client-ip'));
            row.appendChild(elText('span', c.latency_ms.toFixed(1) + 'ms', 'client-lat'));
            container.appendChild(row);
        });
    }

    function renderAlerts(alerts) {
        const container = $('alertList');
        container.replaceChildren();
        if (!alerts.length) {
            container.appendChild(makePlaceholder('All systems healthy', 'ok'));
            return;
        }
        alerts.forEach(a => {
            const sev = a.severity === 'critical' ? 'crit' : a.severity === 'warning' ? 'warn' : 'info';
            const row = el('div', 'alert-row');
            row.appendChild(elText('span', sev === 'info' ? 'i' : '!', 'alert-icon ' + sev));
            row.appendChild(elText('span', a.message, 'alert-msg'));
            row.appendChild(elText('span', fmtTime(a.timestamp), 'alert-time'));
            container.appendChild(row);
        });
    }

    // ── DOM helpers ──
    function el(tag, cls) {
        const e = document.createElement(tag);
        if (cls) e.className = cls;
        return e;
    }

    function elText(tag, text, cls) {
        const e = el(tag, cls);
        e.textContent = text ?? '';
        return e;
    }

    function makePlaceholder(text, extra) {
        const e = el('div', 'placeholder' + (extra ? ' ' + extra : ''));
        e.textContent = text;
        return e;
    }

    // ────────────────────────────────────────────────────────
    //  Events
    // ────────────────────────────────────────────────────────
    function bindEvents() {
        $('failoverBtn').addEventListener('click', () => {
            $('modalBg').classList.add('open');
        });
        $('modalCancel').addEventListener('click', () => {
            $('modalBg').classList.remove('open');
        });
        $('modalBg').addEventListener('click', (e) => {
            if (e.target === $('modalBg')) $('modalBg').classList.remove('open');
        });
        $('modalConfirm').addEventListener('click', () => {
            $('modalBg').classList.remove('open');
            doFailoverSwitch();
        });

        $('autoFailoverToggle').addEventListener('change', (e) => {
            fetch('/api/failover/auto', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ enabled: e.target.checked }),
            }).catch(() => {});
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') $('modalBg').classList.remove('open');
        });
    }

    function doFailoverSwitch() {
        $('failoverBtn').disabled = true;
        $('failoverBtn').textContent = 'Switching\u2026';
        fetch('/api/failover/switch', { method: 'POST' })
            .then(r => r.json())
            .then(d => {
                if (d.success) {
                    $('foHost').textContent = d.active_host;
                    $('foCount').textContent = d.failover_count;
                    $('activeHostPill').textContent = capitalize(d.active_host);
                }
            })
            .catch(() => {})
            .finally(() => {
                $('failoverBtn').disabled = false;
                $('failoverBtn').textContent = 'Switch Host';
            });
    }

    // ────────────────────────────────────────────────────────
    //  Utilities
    // ────────────────────────────────────────────────────────
    function capitalize(s) { return s ? s[0].toUpperCase() + s.slice(1) : ''; }

    function fmtUptime(sec) {
        const d = Math.floor(sec / 86400);
        const h = Math.floor((sec % 86400) / 3600);
        const m = Math.floor((sec % 3600) / 60);
        if (d > 0) return d + 'd ' + h + 'h ' + m + 'm';
        if (h > 0) return h + 'h ' + m + 'm';
        return m + 'm ' + (sec % 60) + 's';
    }

    function fmtTime(ts) {
        if (!ts) return '';
        return new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    }
})();
