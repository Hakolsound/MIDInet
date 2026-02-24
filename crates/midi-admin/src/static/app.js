/* ═══════════════════════════════════════════════════════════
   MIDInet Dashboard — Real-time monitoring
   Vanilla JS · WebSocket · Canvas sparkline
   ═══════════════════════════════════════════════════════════ */
(function () {
    'use strict';

    // ── DOM shortcuts ──
    const $ = (id) => document.getElementById(id);

    // ── Channel display names ──
    const CH_NAMES = {
        midi_in: 'MIDI In',
        midi_out: 'MIDI Out',
        osc: 'OSC',
        api: 'API',
        ws: 'WebSocket',
    };

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
        // Sniffer panel
        snifferWs: null,
        snifferChannel: null,
        snifferAutoScroll: true,
        snifferMaxEntries: 200,
        // Traffic activity recency tracking
        trafficLastActive: {
            midi_in: 0, midi_out: 0, osc: 0, api: 0, ws: 0,
        },
        // Settings
        settingsLoaded: false,
        settingsData: null,
        presets: [],
        // Signal flow data (from REST)
        devices: [],
        activeDevice: null,
        hosts: [],
        clients: [],
        lastStatus: null,
        // Auto-failover confirmation guard
        autoFoPending: false,
        pendingAutoFo: false,
        // Capture mode
        captureWs: null,
        captureType: null,
        captureTimeout: null,
        // Signal flow SVG
        sfSvg: null,
        sfPaths: null,
        sfLabels: null,
        sfParticles: null,
        sfRafId: null,
        sfConnectors: [],
        sfParticlePool: [],
        sfResizeObserver: null,
        sfPrefersReducedMotion: false,
        _sfLastTime: 0,
        _sfLastClientIds: '',
    };

    // ── Bootstrap ──
    document.addEventListener('DOMContentLoaded', () => {
        connectWs();
        bindEvents();
        resizeCanvas();
        initSignalFlowSvg();
        fetchHosts();
        fetchClients();
        fetchAlerts();
        fetchSystemMetrics();
        fetchDevices();
        initInstructions();
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
        S.lastStatus = d;

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

        // Role badge (navbar)
        $('roleBadge').className = 'role-badge ' + ah;
        $('roleLabel').textContent = ah.toUpperCase();

        // Auto-failover indicator (skip if modal is open to avoid fighting)
        if (!S.autoFoPending) {
            const autoEnabled = d.failover?.auto_enabled ?? true;
            const pill = $('autoFailoverPill');
            pill.textContent = autoEnabled ? 'ON' : 'OFF';
            pill.className = 'pill ' + (autoEnabled ? 'on' : 'off');
            $('autoFailoverToggle').checked = autoEnabled;
        }

        // Traffic monitor
        if (d.traffic) {
            $('trafficMidiIn').innerHTML = (d.traffic.midi_in_per_sec ?? 0) + ' <small>pkt/s</small>';
            $('trafficMidiOut').innerHTML = (d.traffic.midi_out_per_sec ?? 0) + ' <small>pkt/s</small>';
            $('trafficOsc').innerHTML = (d.traffic.osc_per_sec ?? 0) + ' <small>msg/s</small>';
            $('trafficApi').innerHTML = (d.traffic.api_per_sec ?? 0) + ' <small>req/s</small>';
            $('trafficWs').innerHTML = (d.traffic.ws_connections ?? 0) + ' <small>conn</small>';
            updateTrafficIndicators(d.traffic);
        }

        // System resources
        updateMeters(d);

        // Uptime
        $('uptimeVal').textContent = fmtUptime(d.uptime ?? 0);

        // Settings sync (for multi-tab awareness)
        if (d.settings && S.settingsLoaded) {
            updateSettingsStatus(d.settings);
        }

        // Signal flow
        updateSignalFlow();
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
        if (!c) return;
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
        if (!c) return;
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
            .then(d => {
                S.hosts = d.hosts || [];
                renderHosts(S.hosts);
                updateSignalFlow();
            })
            .catch(() => {});
        setTimeout(fetchHosts, 5000);
    }

    function fetchClients() {
        fetch('/api/clients')
            .then(r => r.json())
            .then(d => {
                S.clients = d.clients || [];
                renderClients(S.clients);
                updateSignalFlow();
            })
            .catch(() => {});
        setTimeout(fetchClients, 5000);
    }

    function fetchDevices() {
        fetch('/api/devices')
            .then(r => r.json())
            .then(d => {
                S.devices = d.devices || [];
                S.activeDevice = d.active || null;
                updateSignalFlow();
            })
            .catch(() => {});
        setTimeout(fetchDevices, 5000);
    }

    function fetchAlerts() {
        fetch('/api/alerts')
            .then(r => r.json())
            .then(d => renderAlerts(d.active_alerts || []))
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
            const sevLower = (a.severity || '').toLowerCase();
            const sev = sevLower === 'critical' ? 'crit' : sevLower === 'warning' ? 'warn' : 'info';
            const row = el('div', 'alert-row');
            row.appendChild(elText('span', sev === 'info' ? 'i' : '!', 'alert-icon ' + sev));
            row.appendChild(elText('span', a.message, 'alert-msg'));
            row.appendChild(elText('span', fmtTime(a.triggered_at), 'alert-time'));
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
            showModal('Confirm Failover', 'Switch the active host? MIDI output will be briefly interrupted.', 'Switch', doFailoverSwitch);
        });
        $('modalCancel').addEventListener('click', closeModal);
        $('modalBg').addEventListener('click', (e) => {
            if (e.target === $('modalBg')) closeModal();
        });

        $('autoFailoverToggle').addEventListener('change', (e) => {
            // Revert toggle — don't apply until confirmed
            e.target.checked = !e.target.checked;
            const enabling = !e.target.checked;

            $('autoFoModalTitle').textContent = enabling
                ? 'Enable Auto-Failover?'
                : 'Disable Auto-Failover?';
            $('autoFoModalMsg').textContent = enabling
                ? 'The system will automatically switch to the standby host if the primary fails.'
                : 'Manual intervention will be required if the primary host fails.';
            $('autoFoConfirm').textContent = enabling ? 'Enable' : 'Disable';
            $('autoFoConfirm').className = enabling ? 'btn btn-primary' : 'btn btn-warn';

            S.pendingAutoFo = enabling;
            S.autoFoPending = true;
            $('autoFoModalBg').classList.add('open');
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                $('modalBg').classList.remove('open');
                closeAutoFoModal();
                closeSniffer();
            }
        });

        // Auto-failover confirmation modal
        $('autoFoCancel').addEventListener('click', closeAutoFoModal);
        $('autoFoModalBg').addEventListener('click', (e) => {
            if (e.target === $('autoFoModalBg')) closeAutoFoModal();
        });
        $('autoFoConfirm').addEventListener('click', () => {
            $('autoFoModalBg').classList.remove('open');
            S.autoFoPending = false;
            const enabled = S.pendingAutoFo;

            $('autoFailoverToggle').checked = enabled;
            const pill = $('autoFailoverPill');
            pill.textContent = enabled ? 'ON' : 'OFF';
            pill.className = 'pill ' + (enabled ? 'on' : 'off');

            fetch('/api/failover/auto', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ enabled }),
            }).catch(() => {});
        });

        // Traffic sniffer — clickable traffic rows
        document.querySelectorAll('.traffic-row').forEach(row => {
            row.addEventListener('click', () => {
                openSniffer(row.dataset.channel);
            });
        });

        // Sniffer panel close
        $('snifferClose').addEventListener('click', closeSniffer);
        $('snifferBg').addEventListener('click', (e) => {
            if (e.target === $('snifferBg')) closeSniffer();
        });

        // Detect manual scroll in sniffer to pause auto-scroll
        $('snifferLog').addEventListener('scroll', () => {
            const log = $('snifferLog');
            const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 30;
            S.snifferAutoScroll = atBottom;
        });

        // Tab navigation
        document.querySelectorAll('.tab-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
                document.querySelectorAll('.tab-pane').forEach(p => p.classList.remove('active'));
                btn.classList.add('active');
                const pane = document.getElementById('tab-' + btn.dataset.tab);
                if (pane) pane.classList.add('active');
                // Re-measure canvas when switching back to dashboard
                if (btn.dataset.tab === 'dashboard') {
                    setTimeout(resizeCanvas, 50);
                    sfStartAnimation();
                    requestAnimationFrame(sfLayoutPaths);
                } else {
                    sfStopAnimation();
                }
                // Lazy-load settings on first visit
                if (btn.dataset.tab === 'settings' && !S.settingsLoaded) {
                    loadSettings();
                }
            });
        });
    }

    function closeAutoFoModal() {
        $('autoFoModalBg').classList.remove('open');
        S.autoFoPending = false;
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
    //  Instructions tab: IP injection + copy buttons
    // ────────────────────────────────────────────────────────
    function initInstructions() {
        const serverIp = location.hostname;
        const addrEl = $('serverAddr');
        if (addrEl) addrEl.textContent = serverIp + ':8080';

        // Inject server IP into all command placeholders
        document.querySelectorAll('.cmd-ip').forEach(span => {
            span.textContent = serverIp;
        });

        // Copy buttons
        document.querySelectorAll('.btn-copy').forEach(btn => {
            btn.addEventListener('click', () => {
                const code = btn.parentElement.querySelector('code');
                if (!code) return;
                const text = code.textContent;
                if (navigator.clipboard && navigator.clipboard.writeText) {
                    navigator.clipboard.writeText(text).then(() => markCopied(btn)).catch(() => fallbackCopy(code, btn));
                } else {
                    fallbackCopy(code, btn);
                }
            });
        });
    }

    function fallbackCopy(codeEl, btn) {
        const range = document.createRange();
        range.selectNodeContents(codeEl);
        const sel = window.getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        try { document.execCommand('copy'); markCopied(btn); } catch (_) {}
        sel.removeAllRanges();
    }

    function markCopied(btn) {
        btn.textContent = 'Copied!';
        btn.classList.add('copied');
        setTimeout(() => {
            btn.textContent = 'Copy';
            btn.classList.remove('copied');
        }, 2000);
    }

    // ────────────────────────────────────────────────────────
    //  Traffic Sniffer Panel
    // ────────────────────────────────────────────────────────
    function openSniffer(channel) {
        S.snifferChannel = channel;
        S.snifferAutoScroll = true;
        $('snifferTitle').textContent = CH_NAMES[channel] || channel;
        const log = $('snifferLog');
        log.replaceChildren();
        log.appendChild(makePlaceholder('Waiting for traffic\u2026'));
        $('snifferBg').classList.add('open');
        connectSnifferWs();
    }

    function closeSniffer() {
        $('snifferBg').classList.remove('open');
        if (S.snifferWs) {
            S.snifferWs.close();
            S.snifferWs = null;
        }
        S.snifferChannel = null;
    }

    function connectSnifferWs() {
        if (S.snifferWs) {
            S.snifferWs.close();
            S.snifferWs = null;
        }

        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const ws = new WebSocket(proto + '//' + location.host + '/ws/traffic');

        ws.onmessage = (ev) => {
            try {
                const msg = JSON.parse(ev.data);
                if (msg.ch === S.snifferChannel) {
                    appendSnifferRow(msg);
                }
            } catch (e) {
                console.error('sniffer parse', e);
            }
        };

        ws.onclose = () => {
            // Only reconnect if panel is still open
            if ($('snifferBg').classList.contains('open')) {
                setTimeout(connectSnifferWs, 2000);
            }
        };

        ws.onerror = () => ws.close();

        S.snifferWs = ws;
    }

    function appendSnifferRow(msg) {
        const log = $('snifferLog');

        // Remove placeholder on first message
        const ph = log.querySelector('.placeholder');
        if (ph) ph.remove();

        const row = el('div', 'sniffer-row');
        row.appendChild(elText('span', fmtTimeFull(msg.ts), 'sniffer-ts'));
        row.appendChild(elText('span', msg.msg, 'sniffer-msg'));
        log.appendChild(row);

        // Cap entries
        while (log.children.length > S.snifferMaxEntries) {
            log.removeChild(log.firstChild);
        }

        // Auto-scroll
        if (S.snifferAutoScroll) {
            log.scrollTop = log.scrollHeight;
        }
    }

    function fmtTimeFull(ts) {
        if (!ts) return '';
        return new Date(ts * 1000).toLocaleTimeString([], {
            hour: '2-digit',
            minute: '2-digit',
            second: '2-digit',
        });
    }

    // ────────────────────────────────────────────────────────
    //  Traffic Activity Indicators
    // ────────────────────────────────────────────────────────
    function updateTrafficIndicators(traffic) {
        const now = Date.now();
        const channels = {
            midi_in:  traffic.midi_in_per_sec ?? 0,
            midi_out: traffic.midi_out_per_sec ?? 0,
            osc:      traffic.osc_per_sec ?? 0,
            api:      traffic.api_per_sec ?? 0,
            ws:       traffic.ws_connections ?? 0,
        };
        for (const [ch, rate] of Object.entries(channels)) {
            if (rate > 0) S.trafficLastActive[ch] = now;
            const age = now - S.trafficLastActive[ch];
            const row = document.querySelector('.traffic-row[data-channel="' + ch + '"]');
            if (!row) continue;
            const dot = row.querySelector('.traffic-dot');
            if (!dot) continue;
            dot.classList.remove('active', 'recent');
            row.classList.remove('idle');
            if (rate > 0) {
                dot.classList.add('active');
            } else if (age < 5000) {
                dot.classList.add('recent');
            } else {
                row.classList.add('idle');
            }
        }
    }

    // ────────────────────────────────────────────────────────
    //  Signal Flow — Premium SVG Visualization
    // ────────────────────────────────────────────────────────
    var SF_MAX_PARTICLES = 30;

    function initSignalFlowSvg() {
        S.sfSvg = $('sfSvg');
        S.sfPaths = $('sfPaths');
        S.sfLabels = $('sfLabels');
        S.sfParticles = $('sfParticles');

        var mq = window.matchMedia('(prefers-reduced-motion: reduce)');
        S.sfPrefersReducedMotion = mq.matches;
        mq.addEventListener('change', function(e) {
            S.sfPrefersReducedMotion = e.matches;
            if (e.matches) sfStopAnimation(); else sfStartAnimation();
        });

        S.sfResizeObserver = new ResizeObserver(function() {
            sfLayoutPaths();
        });
        S.sfResizeObserver.observe($('signalFlow'));

        requestAnimationFrame(function() {
            sfLayoutPaths();
            sfStartAnimation();
        });
    }

    function sfLayoutPaths() {
        var container = $('signalFlow');
        if (!container || !S.sfPaths) return;
        var rect = container.getBoundingClientRect();

        S.sfPaths.innerHTML = '';
        S.sfLabels.innerHTML = '';
        S.sfConnectors = [];

        var deviceNode = $('sfNodeDevice');
        var hostNode = $('sfNodeHost');
        var monitorNode = $('sfNodeMonitor');
        if (!deviceNode || !hostNode) return;

        function rightEdge(el) {
            var r = el.getBoundingClientRect();
            return { x: r.right - rect.left, y: r.top + r.height / 2 - rect.top };
        }
        function leftEdge(el) {
            var r = el.getBoundingClientRect();
            return { x: r.left - rect.left, y: r.top + r.height / 2 - rect.top };
        }
        function bottomCenter(el) {
            var r = el.getBoundingClientRect();
            return { x: r.left + r.width / 2 - rect.left, y: r.bottom - rect.top };
        }
        function topCenter(el) {
            var r = el.getBoundingClientRect();
            return { x: r.left + r.width / 2 - rect.left, y: r.top - rect.top };
        }

        function makeLine(from, to, id, cls) {
            var line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
            line.setAttribute('x1', from.x);
            line.setAttribute('y1', from.y);
            line.setAttribute('x2', to.x);
            line.setAttribute('y2', to.y);
            line.setAttribute('class', 'sf-path ' + (cls || ''));
            line.setAttribute('marker-end', 'url(#sfArrowIdle)');
            line.id = id;
            S.sfPaths.appendChild(line);
            var length = Math.sqrt(Math.pow(to.x - from.x, 2) + Math.pow(to.y - from.y, 2));
            S.sfConnectors.push({ id: id, from: from, to: to, length: length, element: line, active: false, rate: 0, _spawnAccum: 0 });
            return line;
        }

        function makeLabel(mid, id) {
            var text = document.createElementNS('http://www.w3.org/2000/svg', 'text');
            text.setAttribute('x', mid.x);
            text.setAttribute('y', mid.y - 8);
            text.setAttribute('text-anchor', 'middle');
            text.setAttribute('class', 'sf-rate-label');
            text.id = id;
            S.sfLabels.appendChild(text);
            return text;
        }

        // Device → Host
        var devR = rightEdge(deviceNode);
        var hostL = leftEdge(hostNode);
        makeLine(devR, hostL, 'sfPathDevHost', '');
        makeLabel({ x: (devR.x + hostL.x) / 2, y: (devR.y + hostL.y) / 2 }, 'sfLabelIn');

        // Host → Monitor
        if (monitorNode) {
            var hostR = rightEdge(hostNode);
            var monL = leftEdge(monitorNode);
            makeLine(hostR, monL, 'sfPathHostMon', '');
        }

        // Host → each client
        var clientNodes = document.querySelectorAll('.sf-card-client');
        if (clientNodes.length > 0) {
            var hostB = bottomCenter(hostNode);
            clientNodes.forEach(function(cEl, i) {
                var cT = topCenter(cEl);
                makeLine(hostB, cT, 'sfPathClient' + i, '');
            });
            // Fan-out label
            if (clientNodes.length > 0) {
                var firstC = topCenter(clientNodes[0]);
                makeLabel({ x: (hostB.x + firstC.x) / 2, y: (hostB.y + firstC.y) / 2 }, 'sfLabelOut');
            }
        }
    }

    function sfUpdateConnector(pathId, labelId, active, rate) {
        var pathEl = document.getElementById(pathId);
        if (pathEl) {
            pathEl.classList.toggle('active', active);
            pathEl.setAttribute('marker-end', active ? 'url(#sfArrow)' : 'url(#sfArrowIdle)');
        }
        if (labelId) {
            var labelEl = document.getElementById(labelId);
            if (labelEl) {
                labelEl.textContent = active && rate > 0 ? rate + ' msg/s' : '';
                labelEl.classList.toggle('active', active);
            }
        }
        var conn = S.sfConnectors.find(function(c) { return c.id === pathId; });
        if (conn) { conn.active = active; conn.rate = rate; }
    }

    function sfUpdateReturnPath(focusId) {
        var existing = document.getElementById('sfPathReturn');
        if (existing) existing.remove();

        var returnEl = $('sfReturn');
        var returnText = $('sfReturnText');

        if (focusId != null) {
            var focusClient = S.clients.find(function(c) { return c.id === focusId; });
            returnText.textContent = 'Return: ' + (focusClient ? (focusClient.hostname || focusClient.ip) : 'Client #' + focusId) + ' (focus)';
            returnEl.classList.add('active');

            var focusNode = document.querySelector('.sf-card-client[data-client-id="' + focusId + '"]');
            var deviceNode = $('sfNodeDevice');
            if (focusNode && deviceNode && S.sfPaths) {
                var container = $('signalFlow');
                var rect = container.getBoundingClientRect();
                var fromR = focusNode.getBoundingClientRect();
                var toR = deviceNode.getBoundingClientRect();
                var from = { x: fromR.left + fromR.width / 2 - rect.left, y: fromR.bottom - rect.top + 4 };
                var to = { x: toR.left + toR.width / 2 - rect.left, y: toR.bottom - rect.top + 4 };
                var midY = Math.max(from.y, to.y) + 18;
                var returnPath = document.createElementNS('http://www.w3.org/2000/svg', 'path');
                returnPath.setAttribute('d', 'M' + from.x + ',' + from.y + ' Q' + ((from.x + to.x) / 2) + ',' + midY + ' ' + to.x + ',' + to.y);
                returnPath.setAttribute('class', 'sf-path return-path');
                returnPath.setAttribute('marker-end', 'url(#sfArrowReturn)');
                returnPath.id = 'sfPathReturn';
                S.sfPaths.appendChild(returnPath);
            }
        } else {
            returnText.textContent = 'No return path';
            returnEl.classList.remove('active');
        }
    }

    // Laptop SVG icon for client nodes
    var LAPTOP_SVG = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="4" width="18" height="12" rx="2"/><line x1="2" y1="20" x2="22" y2="20"/></svg>';
    var RING_SVG = '<svg class="sf-ring-svg" viewBox="0 0 56 56"><circle cx="28" cy="28" r="24" fill="none" stroke="var(--sep)" stroke-width="2.5" opacity="0.3"/><circle class="sf-ring-active" cx="28" cy="28" r="24" fill="none" stroke="var(--green)" stroke-width="2.5" stroke-dasharray="150.8" stroke-dashoffset="0" stroke-linecap="round"/></svg>';

    function renderClientNodes(clients, focusId) {
        var newIds = clients.map(function(c) { return c.id + ':' + c.hostname; }).join(',');
        if (newIds === S._sfLastClientIds) {
            // Just update health + focus on existing nodes
            clients.forEach(function(c) {
                var node = document.querySelector('.sf-card-client[data-client-id="' + c.id + '"]');
                if (!node) return;
                var health = (c.packet_loss_percent > 1 || c.latency_ms >= 50) ? 'red' : c.latency_ms >= 10 ? 'amber' : 'green';
                node.dataset.health = c.id === focusId ? '' : health;
                node.classList.toggle('focus', c.id === focusId);
                var sub = node.querySelector('.sf-card-sub');
                if (sub) sub.textContent = c.latency_ms.toFixed(1) + 'ms';
            });
            return false; // no rebuild
        }
        S._sfLastClientIds = newIds;

        var fanout = $('sfFanout');
        fanout.replaceChildren();

        if (clients.length === 0) {
            fanout.appendChild(elText('div', 'No clients', 'sf-fanout-empty'));
            return true; // rebuilt
        }

        var maxShow = 5;
        var shown = clients.slice(0, maxShow);

        shown.forEach(function(c, i) {
            var health = (c.packet_loss_percent > 1 || c.latency_ms >= 50) ? 'red' : c.latency_ms >= 10 ? 'amber' : 'green';
            var isFocus = c.id === focusId;

            var node = el('div', 'sf-card sf-card-client' + (isFocus ? ' focus' : ''));
            node.dataset.health = isFocus ? '' : health;
            node.dataset.clientId = c.id;
            node.style.animationDelay = (0.05 + i * 0.06) + 's';

            var ring = el('div', 'sf-card-ring');
            ring.innerHTML = RING_SVG + '<div class="sf-card-icon">' + LAPTOP_SVG + '</div>';
            node.appendChild(ring);

            var info = el('div', 'sf-card-info');
            info.appendChild(elText('span', c.hostname || c.ip, 'sf-card-label'));
            info.appendChild(elText('span', c.latency_ms.toFixed(1) + 'ms', 'sf-card-sub'));
            node.appendChild(info);

            fanout.appendChild(node);
        });

        if (clients.length > maxShow) {
            fanout.appendChild(elText('div', '+' + (clients.length - maxShow) + ' more', 'sf-fanout-more'));
        }
        return true; // rebuilt
    }

    function updateSignalFlow() {
        var d = S.lastStatus;
        if (!d) return;

        var midiIn = d.traffic?.midi_in_per_sec ?? 0;
        var midiOut = d.traffic?.midi_out_per_sec ?? 0;

        // Crown
        var activeDevice = S.devices.find(function(dev) { return dev.id === S.activeDevice; });
        var crownEl = $('sfCrown');
        if (activeDevice) {
            $('sfDeviceName').textContent = activeDevice.name;
            $('sfDeviceMfr').textContent = activeDevice.manufacturer || '';
            crownEl.classList.toggle('active', activeDevice.connected && midiIn > 0);
        } else if (S.hosts.length > 0 && S.hosts[0].device_name) {
            $('sfDeviceName').textContent = S.hosts[0].device_name;
            $('sfDeviceMfr').textContent = '';
            crownEl.classList.toggle('active', midiIn > 0);
        } else {
            $('sfDeviceName').textContent = 'No MIDI Device';
            $('sfDeviceMfr').textContent = '';
            crownEl.classList.remove('active');
        }

        // Device health
        var deviceHealth = 'gray';
        if (activeDevice) {
            if (activeDevice.connected && midiIn > 0) deviceHealth = 'green';
            else if (activeDevice.connected) deviceHealth = 'amber';
            else deviceHealth = 'red';
        } else if (S.hosts.length > 0) {
            var h = S.hosts.find(function(h) { return h.role === (d.failover?.active_host ?? 'primary'); }) || S.hosts[0];
            if (h.midi_active && midiIn > 0) deviceHealth = 'green';
            else if (h.midi_active) deviceHealth = 'amber';
            else deviceHealth = 'gray';
        }
        $('sfNodeDevice').dataset.health = deviceHealth;
        $('sfDeviceSub').textContent = deviceHealth === 'green' ? 'active' :
            deviceHealth === 'amber' ? 'connected' :
            deviceHealth === 'red' ? 'disconnected' : 'unknown';

        // Host health
        var hostHealth = 'gray';
        var activeRole = d.failover?.active_host ?? 'primary';
        var host = S.hosts.find(function(h) { return h.role === activeRole; }) || S.hosts[0];
        if (host) {
            if (host.heartbeat_ok && host.midi_active) hostHealth = 'green';
            else if (host.heartbeat_ok) hostHealth = 'amber';
            else hostHealth = 'red';
            $('sfHostSub').textContent = host.role || activeRole;
        } else {
            $('sfHostSub').textContent = activeRole;
        }
        $('sfNodeHost').dataset.health = hostHealth;

        // Monitor
        $('sfNodeMonitor').dataset.health = S.connected ? 'green' : 'red';

        // Connectors
        sfUpdateConnector('sfPathDevHost', 'sfLabelIn', midiIn > 0, midiIn);
        sfUpdateConnector('sfPathHostMon', null, S.connected, 0);

        // Client nodes
        var focusId = d.focus_holder;
        var rebuilt = renderClientNodes(S.clients, focusId);

        // Client connectors
        var clientNodes = document.querySelectorAll('.sf-card-client');
        clientNodes.forEach(function(_, i) {
            sfUpdateConnector('sfPathClient' + i, null, midiOut > 0, midiOut);
        });
        // Fan-out label
        sfUpdateConnector(null, 'sfLabelOut', midiOut > 0, midiOut);

        // Return path
        sfUpdateReturnPath(focusId);

        // Relayout SVG if client count changed
        if (rebuilt) {
            requestAnimationFrame(sfLayoutPaths);
        }
    }

    // ── Particle Animation ──
    function sfStartAnimation() {
        if (S.sfRafId) return;
        S._sfLastTime = performance.now();
        S.sfRafId = requestAnimationFrame(sfAnimate);
    }

    function sfStopAnimation() {
        if (S.sfRafId) {
            cancelAnimationFrame(S.sfRafId);
            S.sfRafId = null;
        }
        if (S.sfParticles) S.sfParticles.innerHTML = '';
        S.sfParticlePool = [];
    }

    function sfAnimate(timestamp) {
        var dt = Math.min(timestamp - (S._sfLastTime || timestamp), 50);
        S._sfLastTime = timestamp;

        if (!S.sfPrefersReducedMotion) {
            sfSpawnParticles(dt);
            sfMoveParticles(dt);
        }

        S.sfRafId = requestAnimationFrame(sfAnimate);
    }

    function sfSpawnParticles(dt) {
        S.sfConnectors.forEach(function(conn, idx) {
            if (!conn.active) return;
            conn._spawnAccum = (conn._spawnAccum || 0) + dt;
            var interval = Math.max(120, 600 - Math.min(conn.rate, 500));
            if (conn._spawnAccum >= interval) {
                conn._spawnAccum = 0;
                sfCreateParticle(idx);
            }
        });
    }

    function sfCreateParticle(connIdx) {
        if (S.sfParticlePool.length >= SF_MAX_PARTICLES) {
            var oldest = S.sfParticlePool.shift();
            if (oldest.element.parentNode) oldest.element.parentNode.removeChild(oldest.element);
        }
        var conn = S.sfConnectors[connIdx];
        if (!conn || !S.sfParticles) return;

        var circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
        circle.setAttribute('cx', conn.from.x);
        circle.setAttribute('cy', conn.from.y);
        circle.setAttribute('r', '2.5');
        circle.setAttribute('fill', conn.id === 'sfPathReturn' ? 'var(--purple)' : 'var(--green)');
        circle.setAttribute('opacity', '0.9');
        S.sfParticles.appendChild(circle);

        S.sfParticlePool.push({
            element: circle,
            connIdx: connIdx,
            progress: 0,
            speed: 0.0008 + Math.random() * 0.0004,
        });
    }

    function sfMoveParticles(dt) {
        var toRemove = [];
        S.sfParticlePool.forEach(function(p, i) {
            p.progress += p.speed * dt;
            if (p.progress >= 1) { toRemove.push(i); return; }
            var conn = S.sfConnectors[p.connIdx];
            if (!conn) { toRemove.push(i); return; }
            var x = conn.from.x + (conn.to.x - conn.from.x) * p.progress;
            var y = conn.from.y + (conn.to.y - conn.from.y) * p.progress;
            p.element.setAttribute('cx', x);
            p.element.setAttribute('cy', y);
            if (p.progress > 0.8) {
                var opacity = 1 - ((p.progress - 0.8) / 0.2);
                p.element.setAttribute('opacity', Math.max(0, opacity * 0.9));
            }
        });
        for (var i = toRemove.length - 1; i >= 0; i--) {
            var idx = toRemove[i];
            var p = S.sfParticlePool[idx];
            if (p.element.parentNode) p.element.parentNode.removeChild(p.element);
            S.sfParticlePool.splice(idx, 1);
        }
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

    // ────────────────────────────────────────────────────────
    //  Reusable Modal
    // ────────────────────────────────────────────────────────
    let _modalCallback = null;

    function showModal(title, message, confirmText, onConfirm) {
        $('modalTitle').textContent = title;
        $('modalMessage').textContent = message;
        $('modalConfirm').textContent = confirmText || 'Confirm';
        _modalCallback = onConfirm;
        $('modalConfirm').onclick = () => { closeModal(); if (_modalCallback) _modalCallback(); };
        $('modalBg').classList.add('open');
    }

    function closeModal() {
        $('modalBg').classList.remove('open');
        _modalCallback = null;
    }

    // ────────────────────────────────────────────────────────
    //  Settings Tab
    // ────────────────────────────────────────────────────────

    function loadSettings() {
        S.settingsLoaded = true;
        fetchSettings();
        fetchPresets();
        bindSettingsEvents();
    }

    function fetchSettings() {
        fetch('/api/settings')
            .then(r => r.json())
            .then(d => {
                S.settingsData = d;
                renderSettings(d);
            })
            .catch(() => {});
    }

    function fetchPresets() {
        fetch('/api/settings/presets')
            .then(r => r.json())
            .then(d => {
                S.presets = d.presets || [];
                renderPresets(S.presets, S.settingsData?.active_preset);
            })
            .catch(() => {});
    }

    function renderSettings(d) {
        // MIDI device
        renderMidiDeviceSelect(d.midi_device);
        updateStatusPill('midiDeviceStatusPill', d.midi_device?.status);

        // OSC port
        $('oscPortInput').value = d.osc?.listen_port ?? 8000;
        updateStatusPill('oscPortStatusPill', d.osc?.status);

        // Failover
        const fo = d.failover || {};
        $('foAutoToggle').checked = fo.auto_enabled !== false;
        $('foSwitchBackSelect').value = fo.switch_back_policy || 'manual';
        $('foLockoutInput').value = fo.lockout_seconds ?? 5;
        $('foConfirmSelect').value = fo.confirmation_mode || 'immediate';

        // Heartbeat
        const hb = fo.heartbeat || {};
        $('hbIntervalInput').value = hb.interval_ms ?? 3;
        $('hbThresholdInput').value = hb.miss_threshold ?? 3;
        updateFailoverTimeEstimate();

        // MIDI trigger
        const mt = fo.triggers?.midi || {};
        $('midiTriggerToggle').checked = mt.enabled || false;
        $('midiTrigChannel').value = mt.channel ?? 16;
        $('midiTrigNote').value = mt.note ?? 127;
        $('midiTrigVelocity').value = mt.velocity_threshold ?? 100;
        $('midiTrigGuard').value = mt.guard_note ?? 0;
        toggleSubSection('midiTriggerSub', mt.enabled);

        // OSC trigger
        const ot = fo.triggers?.osc || {};
        $('oscTriggerToggle').checked = ot.enabled || false;
        $('oscTrigPort').value = ot.listen_port ?? 8000;
        $('oscTrigAddress').value = ot.address || '/midinet/failover/switch';
        $('oscTrigSources').value = (ot.allowed_sources || []).join(', ');
        toggleSubSection('oscTriggerSub', ot.enabled);

        // Validate
        validateLockout();
        validateHeartbeatInterval();
        validateMidiTriggerChannel();
        validateOscTriggerSources();
    }

    function renderMidiDeviceSelect(midiDevice) {
        const select = $('midiDeviceSelect');
        select.replaceChildren();

        const devices = midiDevice?.available_devices || [];
        const active = midiDevice?.active_device;

        if (devices.length === 0) {
            const opt = document.createElement('option');
            opt.value = '';
            opt.textContent = 'No devices found';
            select.appendChild(opt);
        }

        // "Auto-detect" option
        const autoOpt = document.createElement('option');
        autoOpt.value = 'auto';
        autoOpt.textContent = 'Auto-detect';
        if (active === 'auto' || !active) autoOpt.selected = true;
        select.appendChild(autoOpt);

        devices.forEach(d => {
            const opt = document.createElement('option');
            opt.value = d.id;
            opt.textContent = d.name + (d.connected ? '' : ' (disconnected)');
            if (d.id === active) opt.selected = true;
            select.appendChild(opt);
        });

        // Show device details
        const details = $('midiDeviceDetails');
        details.replaceChildren();
        const activeDevice = devices.find(d => d.id === active);
        if (activeDevice) {
            const info = el('div', 'setting-row');
            const infoText = el('div', 'setting-info');
            infoText.appendChild(elText('span', 'Manufacturer: ' + activeDevice.manufacturer, 'setting-tip'));
            infoText.appendChild(elText('span', 'Ports: ' + activeDevice.port_count_in + ' in, ' + activeDevice.port_count_out + ' out', 'setting-tip'));
            info.appendChild(infoText);
            details.appendChild(info);
        }
    }

    function renderPresets(presets, activePreset) {
        const grid = $('presetGrid');
        grid.replaceChildren();
        presets.forEach(p => {
            const card = el('div', 'preset-card' + (p.id === activePreset ? ' active' : ''));
            card.dataset.presetId = p.id;
            card.appendChild(elText('div', p.name, 'preset-name'));
            card.appendChild(elText('div', p.description, 'preset-desc'));
            card.addEventListener('click', () => {
                showModal(
                    'Apply Preset: ' + p.name,
                    p.description + ' This will overwrite your current failover settings.',
                    'Apply',
                    () => applyPreset(p.id)
                );
            });
            grid.appendChild(card);
        });
    }

    function updateStatusPill(id, status) {
        const pill = $(id);
        if (!pill) return;
        pill.classList.remove('ok', 'warn', 'err', 'switching');
        switch (status) {
            case 'connected':
            case 'listening':
                pill.textContent = status === 'connected' ? 'Connected' : 'Listening';
                pill.classList.add('ok');
                break;
            case 'switching':
            case 'starting':
                pill.textContent = 'Switching';
                pill.classList.add('switching');
                break;
            case 'error':
                pill.textContent = 'Error';
                pill.classList.add('err');
                break;
            default:
                pill.textContent = status || 'Unknown';
                break;
        }
    }

    function toggleSubSection(id, open) {
        const sub = $(id);
        if (!sub) return;
        if (open) sub.classList.add('open');
        else sub.classList.remove('open');
    }

    function updateFailoverTimeEstimate() {
        const interval = parseInt($('hbIntervalInput').value) || 3;
        const threshold = parseInt($('hbThresholdInput').value) || 3;
        const total = interval * threshold;
        const el = $('foTimeEstimate');
        el.textContent = total + ' ms';
        el.classList.remove('accent', 'ok', 'warn', 'crit');
        if (total <= 15) el.classList.add('ok');
        else if (total <= 50) el.classList.add('warn');
        else el.classList.add('crit');
    }

    // ── Validation ──
    function validateLockout() {
        const val = parseInt($('foLockoutInput').value);
        const warn = $('foLockoutWarning');
        if (val < 1) {
            warn.textContent = 'Lockout of 0s is dangerous \u2014 failover can oscillate with no delay between switches.';
            warn.className = 'setting-warning active crit';
        } else if (val < 3) {
            warn.textContent = 'Values below 3s risk oscillation during power surges or network instability.';
            warn.className = 'setting-warning active warn';
        } else {
            warn.className = 'setting-warning';
        }
    }

    function validateHeartbeatInterval() {
        const val = parseInt($('hbIntervalInput').value);
        const warn = $('hbIntervalWarning');
        if (val < 2) {
            warn.textContent = 'Below 2ms may cause false positives on congested networks.';
            warn.className = 'setting-warning active warn';
        } else {
            warn.className = 'setting-warning';
        }
    }

    function validateMidiTriggerChannel() {
        const val = parseInt($('midiTrigChannel').value);
        const warn = $('midiTrigChannelWarning');
        if (val >= 1 && val <= 10) {
            warn.textContent = 'Channels 1\u201310 may conflict with performance data. Consider 15 or 16.';
            warn.className = 'setting-warning active warn';
        } else {
            warn.className = 'setting-warning';
        }
    }

    function validateOscTriggerSources() {
        const val = ($('oscTrigSources').value || '').trim();
        const warn = $('oscTrigSourcesWarning');
        if ($('oscTriggerToggle').checked && val === '') {
            warn.textContent = 'No source restriction \u2014 any device on the network can trigger failover.';
            warn.className = 'setting-warning active warn';
        } else {
            warn.className = 'setting-warning';
        }
    }

    function validateOscPort() {
        const val = parseInt($('oscPortInput').value);
        const msg = $('oscPortValidation');
        const reserved = [5004, 5005, 5006];
        if (val < 1024) {
            msg.textContent = 'Port must be 1024 or higher (privileged ports require root).';
            msg.className = 'setting-validation active crit';
            return false;
        }
        if (reserved.includes(val)) {
            msg.textContent = 'Port ' + val + ' conflicts with MIDInet data/heartbeat/control channels.';
            msg.className = 'setting-validation active crit';
            return false;
        }
        msg.className = 'setting-validation';
        return true;
    }

    // ── Settings events ──
    function bindSettingsEvents() {
        // MIDI device change
        $('midiDeviceSelect').addEventListener('change', (e) => {
            const deviceId = e.target.value;
            if (!deviceId) return;
            showModal(
                'Switch MIDI Device',
                'Switching the MIDI device will briefly interrupt output (~50ms). The host daemon will pick up the change on config reload.',
                'Switch',
                () => {
                    fetch('/api/settings/midi-device', {
                        method: 'PUT',
                        headers: { 'Content-Type': 'application/json' },
                        body: JSON.stringify({ device_id: deviceId }),
                    })
                        .then(r => r.json())
                        .then(d => {
                            if (d.success) {
                                updateStatusPill('midiDeviceStatusPill', d.status);
                                clearActivePreset();
                            }
                        })
                        .catch(() => {});
                }
            );
        });

        // OSC port apply
        $('oscPortApply').addEventListener('click', () => {
            if (!validateOscPort()) return;
            const port = parseInt($('oscPortInput').value);
            fetch('/api/settings/osc-port', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ port }),
            })
                .then(r => r.json())
                .then(d => {
                    if (d.success) {
                        updateStatusPill('oscPortStatusPill', d.status);
                        const msg = $('oscPortValidation');
                        msg.textContent = 'Port updated to ' + d.port;
                        msg.className = 'setting-validation active ok';
                        setTimeout(() => { msg.className = 'setting-validation'; }, 3000);
                    } else {
                        const msg = $('oscPortValidation');
                        msg.textContent = d.error || 'Failed to change port';
                        msg.className = 'setting-validation active crit';
                    }
                })
                .catch(() => {});
        });

        // OSC port enter key
        $('oscPortInput').addEventListener('keydown', (e) => {
            if (e.key === 'Enter') $('oscPortApply').click();
        });
        $('oscPortInput').addEventListener('input', validateOscPort);

        // Failover save
        $('foSaveBtn').addEventListener('click', saveFailoverSettings);

        // Live validation
        $('foLockoutInput').addEventListener('input', validateLockout);
        $('hbIntervalInput').addEventListener('input', () => {
            validateHeartbeatInterval();
            updateFailoverTimeEstimate();
        });
        $('hbThresholdInput').addEventListener('input', updateFailoverTimeEstimate);
        $('midiTrigChannel').addEventListener('input', validateMidiTriggerChannel);
        $('oscTrigSources').addEventListener('input', validateOscTriggerSources);
        $('oscTriggerToggle').addEventListener('change', validateOscTriggerSources);

        // Collapsible sub-sections
        $('midiTriggerToggle').addEventListener('change', (e) => {
            toggleSubSection('midiTriggerSub', e.target.checked);
        });
        $('oscTriggerToggle').addEventListener('change', (e) => {
            toggleSubSection('oscTriggerSub', e.target.checked);
        });

        // Capture buttons
        $('midiCaptureBtn').addEventListener('click', () => {
            if (S.captureType === 'midi') stopCapture('Cancelled');
            else startCapture('midi');
        });
        $('oscCaptureBtn').addEventListener('click', () => {
            if (S.captureType === 'osc') stopCapture('Cancelled');
            else startCapture('osc');
        });

        // Safe defaults button
        $('safeDefaultsBtn').addEventListener('click', () => {
            showModal(
                'Revert to Safe Defaults',
                'This will reset all failover settings to factory defaults. MIDI device and OSC port are not affected.',
                'Revert',
                () => applyPreset('safe_defaults')
            );
        });
    }

    // ────────────────────────────────────────────────────────
    //  MIDI / OSC Capture
    // ────────────────────────────────────────────────────────
    function startCapture(type) {
        if (S.captureWs) stopCapture();
        S.captureType = type;

        const btn = $(type === 'midi' ? 'midiCaptureBtn' : 'oscCaptureBtn');
        const status = $(type === 'midi' ? 'midiCaptureStatus' : 'oscCaptureStatus');

        btn.classList.add('listening');
        btn.textContent = 'Cancel';
        status.textContent = 'Listening\u2026 send a ' + (type === 'midi' ? 'note' : 'message') + ' now';
        status.className = 'capture-status listening';

        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        S.captureWs = new WebSocket(proto + '//' + location.host + '/ws/traffic');

        S.captureWs.onmessage = function (ev) {
            try {
                var msg = JSON.parse(ev.data);
                if (type === 'midi' && (msg.ch === 'midi_in' || msg.ch === 'midi')) {
                    captureMidiFromTraffic(msg.msg);
                } else if (type === 'osc' && msg.ch === 'osc') {
                    captureOscFromTraffic(msg.msg);
                }
            } catch (e) {}
        };

        S.captureWs.onerror = function () { S.captureWs.close(); };
        S.captureWs.onclose = function () {
            if (S.captureType === type) {
                // Connection lost while still listening — reopen
                setTimeout(function () {
                    if (S.captureType === type) startCapture(type);
                }, 1000);
            }
        };

        // Auto-timeout after 30 s
        S.captureTimeout = setTimeout(function () {
            stopCapture('Timed out \u2014 no message received');
        }, 30000);
    }

    function stopCapture(statusMsg) {
        var type = S.captureType;
        S.captureType = null;

        if (S.captureWs) {
            S.captureWs.onclose = null; // prevent reconnect
            S.captureWs.close();
            S.captureWs = null;
        }
        if (S.captureTimeout) {
            clearTimeout(S.captureTimeout);
            S.captureTimeout = null;
        }

        if (!type) return;

        var btn = $(type === 'midi' ? 'midiCaptureBtn' : 'oscCaptureBtn');
        var status = $(type === 'midi' ? 'midiCaptureStatus' : 'oscCaptureStatus');

        btn.classList.remove('listening');
        btn.textContent = type === 'midi' ? 'Capture Note' : 'Capture Message';

        if (statusMsg) {
            status.textContent = statusMsg;
            status.className = 'capture-status timeout';
            setTimeout(function () { status.className = 'capture-status'; status.textContent = ''; }, 4000);
        }
    }

    function captureMidiFromTraffic(msg) {
        // Traffic format examples:
        //   "Note On ch=16 note=127 vel=100"
        //   "NoteOn Ch16 N127 V100"
        //   "90 7F 64" (raw hex — ch1 note 127 vel 100)
        var chMatch = msg.match(/ch(?:annel)?[=:\s]*(\d+)/i);
        var noteMatch = msg.match(/note[=:\s]*(\d+)/i) || msg.match(/\bN(\d+)\b/);
        var velMatch = msg.match(/vel(?:ocity)?[=:\s]*(\d+)/i) || msg.match(/\bV(\d+)\b/);

        if (noteMatch) {
            if (chMatch) $('midiTrigChannel').value = chMatch[1];
            $('midiTrigNote').value = noteMatch[1];
            if (velMatch) $('midiTrigVelocity').value = velMatch[1];

            var label = (chMatch ? 'Ch' + chMatch[1] + ' ' : '') +
                'Note ' + noteMatch[1] +
                (velMatch ? ' Vel ' + velMatch[1] : '');

            showCaptureSuccess('midi', label);
            validateMidiTriggerChannel();
        }
    }

    function captureOscFromTraffic(msg) {
        // Traffic format: "/some/address [arg1, arg2] from 192.168.1.10"
        var addrMatch = msg.match(/^(\/\S+)/);
        if (addrMatch) {
            $('oscTrigAddress').value = addrMatch[1];
            showCaptureSuccess('osc', addrMatch[1]);
        }
    }

    function showCaptureSuccess(type, label) {
        stopCapture();
        var status = $(type === 'midi' ? 'midiCaptureStatus' : 'oscCaptureStatus');
        status.textContent = 'Captured: ' + label;
        status.className = 'capture-status captured';
        setTimeout(function () { status.className = 'capture-status'; status.textContent = ''; }, 5000);
    }

    function saveFailoverSettings() {
        const btn = $('foSaveBtn');
        btn.disabled = true;
        btn.textContent = 'Saving\u2026';

        const sourcesRaw = ($('oscTrigSources').value || '').trim();
        const sources = sourcesRaw ? sourcesRaw.split(',').map(s => s.trim()).filter(s => s) : [];

        const payload = {
            auto_enabled: $('foAutoToggle').checked,
            switch_back_policy: $('foSwitchBackSelect').value,
            lockout_seconds: parseInt($('foLockoutInput').value) || 5,
            confirmation_mode: $('foConfirmSelect').value,
            heartbeat: {
                interval_ms: parseInt($('hbIntervalInput').value) || 3,
                miss_threshold: parseInt($('hbThresholdInput').value) || 3,
            },
            triggers: {
                midi: {
                    enabled: $('midiTriggerToggle').checked,
                    channel: parseInt($('midiTrigChannel').value) || 16,
                    note: parseInt($('midiTrigNote').value) || 127,
                    velocity_threshold: parseInt($('midiTrigVelocity').value) || 100,
                    guard_note: parseInt($('midiTrigGuard').value) || 0,
                },
                osc: {
                    enabled: $('oscTriggerToggle').checked,
                    listen_port: parseInt($('oscTrigPort').value) || 8000,
                    address: $('oscTrigAddress').value || '/midinet/failover/switch',
                    allowed_sources: sources,
                },
            },
        };

        fetch('/api/settings/failover', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload),
        })
            .then(r => r.json())
            .then(d => {
                if (d.success) {
                    btn.textContent = 'Saved!';
                    clearActivePreset();
                    // Show warnings if any
                    if (d.warnings && d.warnings.length > 0) {
                        setTimeout(() => {
                            alert('Settings saved with warnings:\n\n' + d.warnings.join('\n'));
                        }, 200);
                    }
                } else {
                    btn.textContent = 'Error';
                    alert(d.error || 'Failed to save failover settings');
                }
            })
            .catch(() => {
                btn.textContent = 'Error';
            })
            .finally(() => {
                setTimeout(() => {
                    btn.disabled = false;
                    btn.textContent = 'Save Failover Settings';
                }, 2000);
            });
    }

    function applyPreset(presetId) {
        fetch('/api/settings/preset', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ preset: presetId }),
        })
            .then(r => r.json())
            .then(d => {
                if (d.success) {
                    // Re-render with new settings
                    fetchSettings();
                    highlightPreset(presetId);
                } else {
                    alert(d.error || 'Failed to apply preset');
                }
            })
            .catch(() => {});
    }

    function highlightPreset(activeId) {
        document.querySelectorAll('.preset-card').forEach(card => {
            card.classList.toggle('active', card.dataset.presetId === activeId);
        });
    }

    function clearActivePreset() {
        document.querySelectorAll('.preset-card').forEach(card => {
            card.classList.remove('active');
        });
    }

    // ── Settings sync from WebSocket ──
    function updateSettingsStatus(settings) {
        if (!settings) return;
        updateStatusPill('midiDeviceStatusPill', settings.midi_device_status);
        updateStatusPill('oscPortStatusPill', settings.osc_status);
        if (settings.active_preset) {
            highlightPreset(settings.active_preset);
        }
    }
})();
