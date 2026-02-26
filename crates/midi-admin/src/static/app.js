// ══════════════════════════════════════════════════════════════
// MIDInet Admin Panel — Preact + HTM (no build step)
// ══════════════════════════════════════════════════════════════

import { h, render, createContext } from 'https://esm.sh/preact@10.25.4';
import {
  useState, useEffect, useRef, useReducer,
  useContext, useMemo
} from 'https://esm.sh/preact@10.25.4/hooks';
import htm from 'https://esm.sh/htm@3.1.1';

const html = htm.bind(h);

// ── SVG Icons ─────────────────────────────────────────────────
const svgAttrs = { width: 18, height: 18, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round', 'stroke-linejoin': 'round' };

const ICO = {
  grid: () => html`<svg ...${svgAttrs}><rect x="3" y="3" width="8" height="8" rx="1"/><rect x="13" y="3" width="8" height="8" rx="1"/><rect x="3" y="13" width="8" height="8" rx="1"/><rect x="13" y="13" width="8" height="8" rx="1"/></svg>`,
  sliders: () => html`<svg ...${svgAttrs}><line x1="4" y1="21" x2="4" y2="14"/><line x1="4" y1="10" x2="4" y2="3"/><line x1="12" y1="21" x2="12" y2="12"/><line x1="12" y1="8" x2="12" y2="3"/><line x1="20" y1="21" x2="20" y2="16"/><line x1="20" y1="12" x2="20" y2="3"/><line x1="1" y1="14" x2="7" y2="14"/><line x1="9" y1="8" x2="15" y2="8"/><line x1="17" y1="16" x2="23" y2="16"/></svg>`,
  gear: () => html`<svg ...${svgAttrs}><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 01-2.83 2.83l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z"/></svg>`,
  help: () => html`<svg ...${svgAttrs}><circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 015.83 1c0 2-3 3-3 3"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>`,
  bell: () => html`<svg ...${svgAttrs}><path d="M18 8A6 6 0 006 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.73 21a2 2 0 01-3.46 0"/></svg>`,
  x: () => html`<svg ...${svgAttrs}><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`,
  server: () => html`<svg ...${svgAttrs}><rect x="2" y="2" width="20" height="8" rx="2"/><rect x="2" y="14" width="20" height="8" rx="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/></svg>`,
  users: () => html`<svg ...${svgAttrs}><path d="M17 21v-2a4 4 0 00-4-4H5a4 4 0 00-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 00-3-3.87"/><path d="M16 3.13a4 4 0 010 7.75"/></svg>`,
  usb: () => html`<svg ...${svgAttrs}><rect x="4" y="14" width="16" height="8" rx="2"/><line x1="8" y1="14" x2="8" y2="10"/><line x1="16" y1="14" x2="16" y2="10"/><line x1="12" y1="14" x2="12" y2="6"/><circle cx="12" cy="4" r="2"/></svg>`,
  wifi: () => html`<svg ...${svgAttrs}><path d="M5 12.55a11 11 0 0114.08 0"/><path d="M1.42 9a16 16 0 0121.16 0"/><path d="M8.53 16.11a6 6 0 016.95 0"/><line x1="12" y1="20" x2="12.01" y2="20"/></svg>`,
  search: () => html`<svg ...${svgAttrs}><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>`,
  music: () => html`<svg ...${svgAttrs}><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
  volumeX: () => html`<svg ...${svgAttrs}><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><line x1="23" y1="9" x2="17" y2="15"/><line x1="17" y1="9" x2="23" y2="15"/></svg>`,
  volume: () => html`<svg ...${svgAttrs}><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M19.07 4.93a10 10 0 010 14.14"/><path d="M15.54 8.46a5 5 0 010 7.07"/></svg>`,
};

// ── Utilities ─────────────────────────────────────────────────
const fmtUp = (s) => {
  if (!s) return '0m';
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`;
};
const fmtRate = (v) => v >= 1000 ? (v / 1000).toFixed(1) + 'k' : String(Math.round(v));
const fmtDur = (ms) => { const s = Math.floor(ms / 1000); if (s < 60) return `${s}s`; const m = Math.floor(s / 60); if (m < 60) return `${m}m ${s % 60}s`; return `${Math.floor(m / 60)}h ${m % 60}m`; };
const healthLv = (s) => s >= 80 ? 'ok' : s >= 50 ? 'warn' : 'crit';
const cpuLv = (p) => p < 60 ? 'ok' : p < 85 ? 'warn' : 'crit';
const tempLv = (c) => c < 55 ? 'ok' : c < 70 ? 'warn' : 'crit';
let _tid = 0;
const mkToast = (type, message) => ({ id: ++_tid, type, message, ts: Date.now() });
const copyText = async (text, dispatch) => {
  try { await navigator.clipboard.writeText(text); dispatch({ type: 'ADD_TOAST', toast: mkToast('success', 'Copied') }); }
  catch { dispatch({ type: 'ADD_TOAST', toast: mkToast('error', 'Copy failed') }); }
};
const apiFetch = async (url, opts = {}) => {
  const res = await fetch(url, { headers: { 'Content-Type': 'application/json', ...opts.headers }, ...opts });
  return res.json();
};

// ── State Management ──────────────────────────────────────────
const AppContext = createContext();

const INIT = {
  page: 'overview',
  wsOk: false,
  status: {
    health_score: 0, cpu_percent: 0, cpu_temp_c: 0, memory_used_mb: 0, uptime: 0,
    midi: { messages_per_sec: 0, active_notes: 0, bytes_per_sec: 0 },
    failover: { active_host: 'primary', standby_healthy: false, auto_enabled: true },
    traffic: { midi_in_per_sec: 0, midi_out_per_sec: 0, osc_per_sec: 0, api_per_sec: 0, ws_connections: 0 },
    input_redundancy: { enabled: false, active_input: 0, active_label: 'primary', primary_health: 'unknown', secondary_health: 'unknown', primary_device: '', secondary_device: '', switch_count: 0, auto_switch_enabled: true, last_switch: null },
    host_count: 0, client_count: 0, active_alerts: 0,
    settings: { midi_device_status: 'disconnected', osc_port: 8000, osc_status: 'stopped', active_preset: null },
  },
  hosts: [], clients: [], devices: [], alerts: [],
  designatedPrimary: null, designatedFocus: null,
  pipeline: null, settings: null, presets: [], failoverDetail: null,
  sparkData: [], toasts: [], warningPopups: [],
  trafficLastSeen: { midi_in: 0, midi_out: 0, osc: 0, api: 0 },
  snifferOpen: false, snifferEntries: [], snifferFilter: 'all',
  modal: null,
  deviceActivity: {}, identifyActive: {},
  mutedAlerts: {},  // { [source]: true } — cleared on page reload
};

function reducer(state, action) {
  switch (action.type) {
    case 'SET_PAGE': return { ...state, page: action.page };
    case 'WS_OK': return { ...state, wsOk: action.v };
    case 'WS_STATUS': {
      const d = action.data;
      const spark = [...state.sparkData, d.midi?.messages_per_sec || 0];
      if (spark.length > 120) spark.shift();
      const now = Date.now();
      const tr = d.traffic || {};
      const ls = { ...state.trafficLastSeen };
      if (tr.midi_in_per_sec > 0) ls.midi_in = now;
      if (tr.midi_out_per_sec > 0) ls.midi_out = now;
      if (tr.osc_per_sec > 0) ls.osc = now;
      if (tr.api_per_sec > 0) ls.api = now;
      const da = d.device_activity || state.deviceActivity;
      const ia = {};
      (d.identify_active || []).forEach(id => { ia[id] = true; });
      return {
        ...state, status: { ...state.status, ...d }, sparkData: spark, trafficLastSeen: ls, deviceActivity: da, identifyActive: ia,
        hosts: d.hosts || state.hosts,
        clients: d.clients || state.clients,
        designatedPrimary: d.designated_primary !== undefined ? d.designated_primary : state.designatedPrimary,
        designatedFocus: d.designated_focus !== undefined ? d.designated_focus : state.designatedFocus,
      };
    }
    case 'SET_HOSTS': return { ...state, hosts: action.data || [] };
    case 'SET_CLIENTS': return { ...state, clients: action.data || [] };
    case 'SET_DESIGNATED_PRIMARY': return { ...state, designatedPrimary: action.id };
    case 'SET_DESIGNATED_FOCUS': return { ...state, designatedFocus: action.id };
    case 'SET_DEVICES': return { ...state, devices: action.data || [] };
    case 'SET_ALERTS': {
      const next = action.data || [];
      const prevKeys = state.alerts.map(a => a.source + ':' + a.state).sort().join(',');
      const nextKeys = next.map(a => a.source + ':' + a.state).sort().join(',');
      if (prevKeys === nextKeys) return state;
      return { ...state, alerts: next };
    }
    case 'SET_PIPELINE': return { ...state, pipeline: action.data };
    case 'SET_SETTINGS': return { ...state, settings: action.data };
    case 'SET_PRESETS': return { ...state, presets: action.data || [] };
    case 'SET_FAILOVER': return { ...state, failoverDetail: action.data };
    case 'ADD_TOAST': return { ...state, toasts: [...state.toasts, action.toast].slice(-5) };
    case 'RM_TOAST': return { ...state, toasts: state.toasts.filter(t => t.id !== action.id) };
    case 'WARNING_SHOW': {
      const filtered = state.warningPopups.filter(w => w.alertSource !== action.popup.alertSource);
      return { ...state, warningPopups: [...filtered, action.popup].slice(-3) };
    }
    case 'WARNING_DISMISS': return { ...state, warningPopups: state.warningPopups.filter(w => w.alertSource !== action.source) };
    case 'MUTE_ALERT': return { ...state, mutedAlerts: { ...state.mutedAlerts, [action.source]: true }, warningPopups: state.warningPopups.filter(w => w.alertSource !== action.source) };
    case 'UNMUTE_ALERT': { const m = { ...state.mutedAlerts }; delete m[action.source]; return { ...state, mutedAlerts: m }; }
    case 'SNIFFER_OPEN': return { ...state, snifferOpen: true, snifferEntries: [] };
    case 'SNIFFER_CLOSE': return { ...state, snifferOpen: false };
    case 'SNIFFER_ENTRY': {
      const e = [...state.snifferEntries, action.entry]; if (e.length > 500) e.shift();
      return { ...state, snifferEntries: e };
    }
    case 'SNIFFER_FILTER': return { ...state, snifferFilter: action.f };
    case 'MODAL': return { ...state, modal: action.modal };
    case 'MODAL_CLOSE': return { ...state, modal: null };
    case 'DEVICE_ACTIVITY_SNAPSHOT': return { ...state, deviceActivity: action.data || {} };
    case 'DEVICE_ACTIVITY_UPDATE': {
      const u = action.data;
      return { ...state, deviceActivity: { ...state.deviceActivity, [u.device_id]: u } };
    }
    case 'IDENTIFY_START': return { ...state, identifyActive: { ...state.identifyActive, [action.deviceId]: true } };
    case 'IDENTIFY_END': {
      const next = { ...state.identifyActive }; delete next[action.deviceId];
      return { ...state, identifyActive: next };
    }
    default: return state;
  }
}

// ── Custom Hooks ──────────────────────────────────────────────
function useWS(url, onMsg, onStatus) {
  useEffect(() => {
    let ws, timer, delay = 1000, active = true;
    const connect = () => {
      const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
      ws = new WebSocket(`${proto}//${location.host}${url}`);
      ws.onopen = () => { delay = 1000; onStatus?.(true); };
      ws.onmessage = (e) => { try { onMsg(JSON.parse(e.data)); } catch {} };
      ws.onclose = () => { onStatus?.(false); if (active) { timer = setTimeout(connect, delay); delay = Math.min(delay * 1.5, 10000); } };
      ws.onerror = () => ws.close();
    };
    connect();
    return () => { active = false; clearTimeout(timer); ws?.close(); };
  }, [url]);
}

function usePoll(url, ms, cb) {
  useEffect(() => {
    let on = true;
    const go = () => fetch(url).then(r => r.json()).then(d => { if (on) cb(d); }).catch(() => {});
    go(); const id = setInterval(go, ms);
    return () => { on = false; clearInterval(id); };
  }, [url, ms]);
}

function useSparkline(ref, data, color = '#0a84ff') {
  useEffect(() => {
    const c = ref.current;
    if (!c || data.length < 2) return;
    const ctx = c.getContext('2d'), dpr = devicePixelRatio || 1;
    const w = c.clientWidth, h = c.clientHeight;
    c.width = w * dpr; c.height = h * dpr; ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);
    const max = Math.max(...data, 1), step = w / (data.length - 1);
    const y = (v) => h - (v / max) * h * 0.85 - 2;
    const grad = ctx.createLinearGradient(0, 0, 0, h);
    grad.addColorStop(0, color + '28'); grad.addColorStop(1, color + '03');
    ctx.beginPath(); ctx.moveTo(0, h);
    data.forEach((v, i) => ctx.lineTo(i * step, y(v)));
    ctx.lineTo(w, h); ctx.closePath(); ctx.fillStyle = grad; ctx.fill();
    ctx.beginPath();
    data.forEach((v, i) => { i === 0 ? ctx.moveTo(i * step, y(v)) : ctx.lineTo(i * step, y(v)); });
    ctx.strokeStyle = color; ctx.lineWidth = 1.5; ctx.stroke();
    const lx = (data.length - 1) * step, ly = y(data[data.length - 1]);
    ctx.beginPath(); ctx.arc(lx, ly, 3, 0, Math.PI * 2); ctx.fillStyle = color; ctx.fill();
  }, [data, color]);
}

// ── Header ────────────────────────────────────────────────────
function alertAge(ts) {
  if (!ts) return { label: '', color: 'var(--text-3)' };
  const secs = Math.floor(Date.now() / 1000) - ts;
  const color = secs < 60 ? 'var(--green)' : secs < 600 ? 'var(--orange)' : 'var(--text-3)';
  let label;
  if (secs < 60) label = 'just now';
  else if (secs < 3600) label = Math.floor(secs / 60) + 'm ago';
  else if (secs < 86400) label = Math.floor(secs / 3600) + 'h ago';
  else label = Math.floor(secs / 86400) + 'd ago';
  return { label, color };
}

function Header() {
  const { state, dispatch } = useContext(AppContext);
  const [alertsOpen, setAlertsOpen] = useState(false);
  const s = state.status;
  const role = s.failover?.active_host || 'primary';
  const lv = healthLv(s.health_score);
  const tabs = [
    { id: 'overview', label: 'Overview' },
    { id: 'control', label: 'Control' },
    { id: 'settings', label: 'Settings' },
    { id: 'help', label: 'Help' },
  ];
  const alerts = state.alerts || [];
  return html`<header class="header">
    <div class="header-brand">
      <img src="/logo.png" width="24" height="24" alt="hakol" style="border-radius:6px" />
      MIDInet
    </div>
    <span class="header-conn" data-ok=${String(state.wsOk)} />
    <nav class="nav-tabs">
      ${tabs.map(t => html`
        <button class="nav-tab ${state.page === t.id ? 'active' : ''}" key=${t.id}
          onClick=${() => { window.location.hash = '#' + t.id; }}>${t.label}</button>
      `)}
    </nav>
    <div class="header-spacer" />
    <div class="header-role" data-role=${role}>${role.toUpperCase()}</div>
    <div class="header-health">
      <span class="header-health-value" style="color:var(--${lv === 'ok' ? 'green' : lv === 'warn' ? 'orange' : 'red'})">${s.health_score}</span>
      <span class="header-health-pts" style="color:var(--${lv === 'ok' ? 'green' : lv === 'warn' ? 'orange' : 'red'})">pts</span>
    </div>
    <button class="header-alerts" onClick=${() => setAlertsOpen(!alertsOpen)}>
      ${ICO.bell()}
      ${s.active_alerts > 0 && html`<span class="header-alerts-badge">${s.active_alerts}</span>`}
      ${alertsOpen && html`
        <div class="alerts-dropdown" onClick=${(e) => e.stopPropagation()}>
          <div class="alerts-dropdown-header">
            Alerts
            <span>${alerts.length} active</span>
          </div>
          <div class="alerts-dropdown-list">
            ${alerts.length === 0 && html`<div class="alerts-dropdown-empty">No active alerts</div>`}
            ${alerts.map(a => {
              const age = alertAge(a.triggered_at);
              const muted = !!state.mutedAlerts[a.source];
              return html`
              <div class="alert-item ${muted ? 'alert-item-muted' : ''}" key=${a.id}>
                <div class="alert-item-dot" data-sev=${a.severity} />
                <div class="alert-item-body">
                  <div class="alert-item-title">${a.title}${muted && html`<span class="alert-muted-badge">muted</span>`}</div>
                  <div class="alert-item-msg">${a.message}</div>
                </div>
                <button class="alert-item-mute" title=${muted ? 'Unmute' : 'Mute'} onClick=${(e) => { e.stopPropagation(); dispatch({ type: muted ? 'UNMUTE_ALERT' : 'MUTE_ALERT', source: a.source }); }}>
                  ${muted ? ICO.volume() : ICO.volumeX()}
                </button>
                <div class="alert-item-time" style="color:${age.color}">${age.label}</div>
              </div>`;
            })}
          </div>
        </div>
      `}
    </button>
    ${alertsOpen && html`<div class="alerts-overlay" onClick=${() => setAlertsOpen(false)} />`}
  </header>`;
}

// ── Footer ────────────────────────────────────────────────────
function Footer() {
  const { state } = useContext(AppContext);
  const s = state.status;
  return html`<footer class="footer">
    <div class="footer-item"><span class="footer-label">CPU</span><span class="footer-val" data-lv=${cpuLv(s.cpu_percent)}>${Math.round(s.cpu_percent)}%</span></div>
    <span class="footer-sep" />
    <div class="footer-item"><span class="footer-label">Temp</span><span class="footer-val" data-lv=${tempLv(s.cpu_temp_c)}>${Math.round(s.cpu_temp_c)}°C</span></div>
    <span class="footer-sep" />
    <div class="footer-item"><span class="footer-label">RAM</span><span class="footer-val">${s.memory_used_mb}MB</span></div>
    <span class="footer-sep" />
    <div class="footer-item"><span class="footer-label">MIDI</span><span class="footer-val" data-lv="data">${fmtRate(s.midi?.messages_per_sec || 0)}/s</span></div>
    <div class="footer-spacer" />
    <div class="footer-item"><span class="footer-label">Up</span><span class="footer-val">${fmtUp(s.uptime)}</span></div>
  </footer>`;
}

// ── Overlays ──────────────────────────────────────────────────
function ConfirmModal() {
  const { state, dispatch } = useContext(AppContext);
  const m = state.modal;
  if (!m) return null;
  const close = () => dispatch({ type: 'MODAL_CLOSE' });
  const confirm = () => { m.onConfirm?.(); close(); };
  return html`<div class="modal-backdrop open" onClick=${close}>
    <div class="modal" onClick=${(e) => e.stopPropagation()}>
      <div class="modal-header">${m.title}</div>
      <div class="modal-body">${m.message}</div>
      <div class="modal-footer">
        <button class="btn" onClick=${close}>Cancel</button>
        <button class="btn ${m.cls || 'btn-accent'}" onClick=${confirm}>${m.ok || 'Confirm'}</button>
      </div>
    </div>
  </div>`;
}

// ── Warning Popups (buzzing seatbelt) ────────────────────────
function useWarningPopups(alerts, mutedAlerts, dispatch) {
  const trackRef = useRef({});  // { [source]: { firstSeen, lastShown, showCount } }

  useEffect(() => {
    const list = alerts || [];
    const ts = trackRef.current;
    const now = Date.now();
    const activeSources = new Set(list.map(a => a.source));

    // Resolved alerts — fire success toast
    for (const src of Object.keys(ts)) {
      if (!activeSources.has(src)) {
        const dur = fmtDur(now - ts[src].firstSeen);
        dispatch({ type: 'ADD_TOAST', toast: mkToast('success', `Resolved: ${src.replace(/_/g, ' ')} (was active ${dur})`) });
        delete ts[src];
      }
    }

    // New alerts — show immediately (unless muted)
    for (const alert of list) {
      if (!ts[alert.source]) {
        ts[alert.source] = { firstSeen: now, lastShown: now, showCount: 1 };
        if (!mutedAlerts[alert.source]) _showWarning(alert, ts[alert.source], dispatch);
      }
    }
  }, [alerts, mutedAlerts]);

  // 1s tick for re-appearances
  useEffect(() => {
    const id = setInterval(() => {
      const list = alerts || [];
      const ts = trackRef.current;
      const now = Date.now();
      for (const alert of list) {
        if (mutedAlerts[alert.source]) continue;
        const t = ts[alert.source];
        if (!t) continue;
        const interval = alert.severity === 'Critical' ? 15000 : 30000;
        if (now - t.lastShown >= interval) {
          t.lastShown = now;
          t.showCount += 1;
          _showWarning(alert, t, dispatch);
        }
      }
    }, 1000);
    return () => clearInterval(id);
  }, [alerts, mutedAlerts]);
}

function _showWarning(alert, tracking, dispatch) {
  const dur = fmtDur(Date.now() - tracking.firstSeen);
  const n = tracking.showCount;
  let message = alert.message;
  if (n >= 4) message = `${alert.message} \u2014 active for ${dur} (warned ${n}x)`;
  else if (n > 1) message = `${alert.message} \u2014 active for ${dur}`;

  dispatch({ type: 'WARNING_SHOW', popup: {
    alertSource: alert.source, severity: alert.severity,
    title: alert.title, message, showCount: n, duration: dur, createdAt: Date.now(),
  }});

  setTimeout(() => dispatch({ type: 'WARNING_DISMISS', source: alert.source }), 5000);
}

function WarningPopupContainer() {
  const { state, dispatch } = useContext(AppContext);
  const popups = state.warningPopups || [];
  if (!popups.length) return null;
  return html`<div class="warning-popup-container">
    ${popups.map(p => html`
      <div class="warning-popup" data-sev=${p.severity} key=${p.alertSource}>
        <div class="warning-popup-header">
          <span class="warning-popup-dot" data-sev=${p.severity} />
          <span class="warning-popup-title">${p.title}</span>
          <button class="warning-popup-mute" title="Mute this alert" onClick=${() => dispatch({ type: 'MUTE_ALERT', source: p.alertSource })}>
            ${ICO.volumeX()}
          </button>
          <button class="warning-popup-close" onClick=${() => dispatch({ type: 'WARNING_DISMISS', source: p.alertSource })}>
            ${ICO.x()}
          </button>
        </div>
        <div class="warning-popup-body">${p.message}</div>
        ${p.showCount > 1 && html`<div class="warning-popup-meta">Active for ${p.duration}</div>`}
        <div class="warning-popup-progress" data-sev=${p.severity} />
      </div>
    `)}
  </div>`;
}

function ToastContainer() {
  const { state, dispatch } = useContext(AppContext);
  useEffect(() => {
    if (!state.toasts.length) return;
    const t = state.toasts[0];
    const rem = Math.max(4000 - (Date.now() - t.ts), 100);
    const id = setTimeout(() => dispatch({ type: 'RM_TOAST', id: t.id }), rem);
    return () => clearTimeout(id);
  }, [state.toasts]);
  return html`<div class="toast-container">
    ${state.toasts.map(t => html`<div class="toast" data-type=${t.type} key=${t.id}><span class="toast-dot" />${t.message}</div>`)}
  </div>`;
}

function SnifferDrawer() {
  const { state, dispatch } = useContext(AppContext);
  const bodyRef = useRef(null);
  useEffect(() => {
    if (!state.snifferOpen) return;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/ws/traffic`);
    ws.onmessage = (e) => { try { dispatch({ type: 'SNIFFER_ENTRY', entry: JSON.parse(e.data) }); } catch {} };
    return () => ws.close();
  }, [state.snifferOpen]);
  useEffect(() => { if (bodyRef.current) bodyRef.current.scrollTop = bodyRef.current.scrollHeight; }, [state.snifferEntries.length]);
  const list = state.snifferFilter === 'all' ? state.snifferEntries : state.snifferEntries.filter(e => e.ch === state.snifferFilter);
  const chColor = { midi: 'accent', osc: 'green', api: 'orange', ws: 'text-3' };
  return html`<div class="sniffer-backdrop ${state.snifferOpen ? 'open' : ''}" onClick=${() => dispatch({ type: 'SNIFFER_CLOSE' })}>
    <div class="sniffer-panel" onClick=${(e) => e.stopPropagation()}>
      <div class="sniffer-header">
        <span class="sniffer-title">Traffic Sniffer</span>
        <div class="flex items-center gap-sm">
          <select value=${state.snifferFilter} onChange=${(e) => dispatch({ type: 'SNIFFER_FILTER', f: e.target.value })}>
            <option value="all">All</option><option value="midi">MIDI</option><option value="osc">OSC</option><option value="api">API</option><option value="ws">WS</option>
          </select>
          <button class="btn btn-sm btn-icon" onClick=${() => dispatch({ type: 'SNIFFER_CLOSE' })}>${ICO.x()}</button>
        </div>
      </div>
      <div class="sniffer-body" ref=${bodyRef}>
        ${list.length === 0 && html`<div class="empty-state">Waiting for traffic...</div>`}
        ${list.map((e, i) => html`<div class="sniffer-line" key=${i}>
          <span class="sniffer-ts">${new Date(e.ts * 1000).toLocaleTimeString()}</span>
          <span style="color:var(--${chColor[e.ch] || 'text-3'});width:36px;font-size:10px;text-transform:uppercase;font-weight:600">${e.ch}</span>
          <span class="sniffer-msg">${e.msg}</span>
        </div>`)}
      </div>
    </div>
  </div>`;
}

// ── Overview Page ─────────────────────────────────────────────
function OverviewPage() {
  return html`<div class="overview-grid">
    <${ControllersCard} />
    <${MidiDataCard} />
    <${NetworkCard} />
    <${ClientsCard} />
  </div>`;
}

function ControllersCard() {
  const { state, dispatch } = useContext(AppContext);
  const midi = state.status.midi || {};
  const ir = state.status.input_redundancy || {};
  const hMap = { active: 'ok', disconnected: 'error', error: 'error', reconnecting: 'warn', unknown: 'idle' };

  // Determine which device is Active vs Backup based on active_input
  const activeIdx = ir.active_input || 0;
  const activeName = activeIdx === 0 ? (ir.primary_device || 'Primary') : (ir.secondary_device || 'Secondary');
  const activeHealth = activeIdx === 0 ? ir.primary_health : ir.secondary_health;
  const backupName = activeIdx === 0 ? (ir.secondary_device || 'Secondary') : (ir.primary_device || 'Primary');
  const backupHealth = activeIdx === 0 ? ir.secondary_health : ir.primary_health;

  const doSwitch = async () => {
    const r = await apiFetch('/api/input-redundancy/switch', { method: 'POST' });
    if (!r.success) dispatch({ type: 'ADD_TOAST', toast: mkToast('error', r.error || 'Switch failed') });
  };
  const toggleAuto = async () => {
    await apiFetch('/api/input-redundancy/auto', {
      method: 'POST', body: JSON.stringify({ enabled: !ir.auto_switch_enabled })
    });
  };

  // Source badge for last switch event
  const ls = ir.last_switch;
  const sourceBadge = (trigger) => {
    if (!trigger) return '';
    const map = { health: 'auto', activity_timeout: 'auto', api: 'manual', osc: 'osc', midi: 'midi' };
    const label = map[trigger] || trigger;
    return label.toUpperCase();
  };
  const sourceClass = (trigger) => {
    if (!trigger) return '';
    if (trigger === 'health' || trigger === 'activity_timeout') return 'source-auto';
    if (trigger === 'api') return 'source-manual';
    if (trigger === 'osc') return 'source-osc';
    return 'source-manual';
  };

  return html`<div class="card">
    <div class="card-header">
      <span class="card-header-icon">${ICO.usb()}</span>
      Controllers
      ${ir.enabled && html`<label class="auto-toggle" title=${ir.auto_switch_enabled ? 'Auto-switch enabled' : 'Auto-switch disabled'}>
        <input type="checkbox" checked=${ir.auto_switch_enabled} onChange=${toggleAuto} />
        <span class="auto-toggle-label">Auto</span>
      </label>`}
    </div>
    <div class="card-body">
      <div class="input-red">
        <div class="input-red-item active">
          <div class="input-red-icon">${ICO.usb()}</div>
          <div class="input-red-info">
            <div class="input-red-label">Active</div>
            <div class="input-red-device">${activeName}</div>
            <div class="input-red-status">
              <span class="status-dot" data-status=${hMap[activeHealth] || 'idle'} />${activeHealth || 'unknown'}
              <span class="mono" style="margin-left:auto">${activeHealth === 'active' ? fmtRate(midi.messages_per_sec || 0) + '/s' : ''}</span>
            </div>
          </div>
        </div>
        <div class="input-red-item">
          <div class="input-red-icon">${ICO.usb()}</div>
          <div class="input-red-info">
            <div class="input-red-label">Backup</div>
            <div class="input-red-device">${backupName}</div>
            <div class="input-red-status">
              <span class="status-dot" data-status=${hMap[backupHealth] || 'idle'} />${backupHealth || 'unknown'}
            </div>
          </div>
        </div>
      </div>
      ${ir.enabled && html`<button class="switch-btn" onClick=${doSwitch}>SWITCH</button>`}
      ${ls && html`<div class="switch-event">
        <span class="switch-source ${sourceClass(ls.trigger)}">${sourceBadge(ls.trigger)}</span>
        <span class="mono" style="font-size:10px;color:var(--text-3)">${ir.switch_count} switch${ir.switch_count !== 1 ? 'es' : ''}</span>
      </div>`}
    </div>
  </div>`;
}

function MidiDataCard() {
  const { state } = useContext(AppContext);
  const ref = useRef(null);
  useSparkline(ref, state.sparkData);
  const midi = state.status.midi || {};
  return html`<div class="card">
    <div class="card-header">
      <span class="card-header-icon">${ICO.music()}</span>
      MIDI Data
    </div>
    <div class="card-body-flush" style="flex:1;min-height:0;padding:8px 12px">
      <div class="spark-wrap"><canvas ref=${ref} /></div>
    </div>
    <div class="midi-stats">
      <div class="midi-stat">
        <span class="midi-stat-value">${fmtRate(midi.messages_per_sec || 0)}</span>
        <span class="midi-stat-label">msg/s</span>
      </div>
      <div class="midi-stat">
        <span class="midi-stat-value">${midi.active_notes || 0}</span>
        <span class="midi-stat-label">notes</span>
      </div>
      <div class="midi-stat">
        <span class="midi-stat-value">${midi.bytes_per_sec ? (midi.bytes_per_sec / 1024).toFixed(1) : '0'}</span>
        <span class="midi-stat-label">KB/s</span>
      </div>
    </div>
  </div>`;
}

function trafficColor(lastSeen) {
  if (!lastSeen) return 'var(--text-3)';
  const age = Date.now() - lastSeen;
  if (age < 3000) return 'var(--green)';
  if (age < 15000) return 'var(--orange)';
  return 'var(--text-3)';
}

function NetworkCard() {
  const { state, dispatch } = useContext(AppContext);
  const t = state.status.traffic || {};
  const ls = state.trafficLastSeen;
  const mx = Math.max(t.midi_in_per_sec || 0, t.midi_out_per_sec || 0, t.osc_per_sec || 0, t.api_per_sec || 0, 1);
  const chs = [
    { k: 'midi_in', l: 'MIDI In', v: t.midi_in_per_sec || 0 },
    { k: 'midi_out', l: 'MIDI Out', v: t.midi_out_per_sec || 0 },
    { k: 'osc', l: 'OSC', v: t.osc_per_sec || 0 },
    { k: 'api', l: 'API', v: t.api_per_sec || 0 },
  ];
  return html`<div class="card">
    <div class="card-header">
      <span class="card-header-icon">${ICO.wifi()}</span>
      Network
      <div class="card-header-right">
        <button class="btn btn-sm" onClick=${() => dispatch({ type: 'SNIFFER_OPEN' })}>${ICO.search()} Sniffer</button>
      </div>
    </div>
    <div class="card-body">
      ${state.hosts.length > 0 && html`
        <div class="ctrl-section-label">Hosts</div>
        ${state.hosts.map(h => {
          const isMaster = state.designatedPrimary === h.id;
          return html`<div class="host-row" key=${h.id}>
            <span class="status-dot" data-status=${h.heartbeat_ok ? 'ok' : 'error'} />
            <span class="host-name">${h.device_name || h.name || h.ip}</span>
            <span class="host-role-badge" data-role=${isMaster ? 'primary' : h.role}>${isMaster ? 'master' : h.role}</span>
            <span class="host-detail">${h.ip}</span>
            <button class="btn btn-xs ${isMaster ? 'btn-active' : ''}" onClick=${() =>
              apiFetch('/api/hosts/' + h.id + '/role', { method: 'PUT', body: JSON.stringify({ role: 'primary' }) })
                .then(() => dispatch({ type: 'SET_DESIGNATED_PRIMARY', id: h.id }))
            }>${isMaster ? '\u2605 Master' : 'Set Master'}</button>
          </div>`;
        })}
      `}
      ${state.hosts.length === 0 && html`<div style="font-size:12px;color:var(--text-3);margin-bottom:12px">No hosts discovered</div>`}
      <div class="ctrl-section-label" style="margin-top:12px">Traffic</div>
      ${chs.map(c => {
        const clr = trafficColor(ls[c.k]);
        return html`<div class="traffic-row" key=${c.k}>
          <span class="traffic-ch" style="color:${clr}">${c.l}</span>
          <div class="traffic-bar-wrap"><div class="traffic-bar" style="background:${clr};width:${Math.min((c.v / mx) * 100, 100)}%" /></div>
          <span class="traffic-val" style="color:${clr}">${fmtRate(c.v)}/s</span>
        </div>`;
      })}
    </div>
  </div>`;
}

function ClientsCard() {
  const { state, dispatch } = useContext(AppContext);
  const [showAdd, setShowAdd] = useState(false);
  const [newIp, setNewIp] = useState('');
  const [newHost, setNewHost] = useState('');

  const addClient = () => {
    if (!newIp.trim()) return;
    apiFetch('/api/clients/add', { method: 'POST', body: JSON.stringify({ ip: newIp.trim(), hostname: newHost.trim() }) })
      .then(d => { if (d.success) { setNewIp(''); setNewHost(''); setShowAdd(false); } else { alert(d.error || 'Failed'); } });
  };
  const removeClient = (id) => {
    apiFetch('/api/clients/' + id, { method: 'DELETE' });
  };

  return html`<div class="card">
    <div class="card-header">
      <span class="card-header-icon">${ICO.users()}</span>
      Clients
      <div class="card-header-right">
        <span style="font-weight:400;color:var(--text-3);font-size:12px;margin-right:8px">${state.clients.length}</span>
        <button class="btn btn-xs" onClick=${() => setShowAdd(!showAdd)}>+ Add</button>
      </div>
    </div>
    <div class="card-body-flush" style="overflow-y:auto">
      ${showAdd && html`<div style="padding:8px 20px;display:flex;gap:6px;align-items:center;border-bottom:1px solid var(--border)">
        <input type="text" placeholder="IP Address" value=${newIp} onInput=${e => setNewIp(e.target.value)}
          style="flex:1;padding:4px 8px;background:var(--bg-2);border:1px solid var(--border);border-radius:4px;color:var(--text-1);font-size:12px" />
        <input type="text" placeholder="Hostname" value=${newHost} onInput=${e => setNewHost(e.target.value)}
          style="flex:1;padding:4px 8px;background:var(--bg-2);border:1px solid var(--border);border-radius:4px;color:var(--text-1);font-size:12px" />
        <button class="btn btn-xs btn-active" onClick=${addClient}>Add</button>
      </div>`}
      ${state.clients.length === 0 && !showAdd && html`<div class="empty-state">No clients connected</div>`}
      <div style="padding:4px 20px">
        ${state.clients.map(c => {
          const hasFocus = state.designatedFocus === c.id;
          const connStatus = c.connection_state === 'connected' ? 'ok' : c.connection_state === 'manual' ? 'warn' : c.connection_state === 'discovering' ? 'warn' : 'error';
          return html`<div class="client-row" key=${c.id}>
            <span class="status-dot" data-status=${connStatus} />
            <span class="client-name">${c.hostname || 'Client ' + c.id}</span>
            <span class="client-ip">${c.ip}</span>
            <span class="client-stat">${c.device_ready ? (c.device_name || 'Ready') : c.manual ? 'Manual' : 'No device'}</span>
            <span class="client-stat">${c.latency_ms?.toFixed(1) || '—'}ms</span>
            <span class="client-stat" style="color:var(--${c.packet_loss_percent > 1 ? 'red' : c.packet_loss_percent > 0.1 ? 'orange' : 'text-3'})">${c.packet_loss_percent?.toFixed(2) || '0'}%</span>
            <button class="btn btn-xs ${hasFocus ? 'btn-active' : ''}" onClick=${() =>
              apiFetch('/api/clients/' + c.id + '/focus', { method: 'PUT', body: JSON.stringify({ focus: !hasFocus }) })
                .then(() => dispatch({ type: 'SET_DESIGNATED_FOCUS', id: hasFocus ? null : c.id }))
            }>${hasFocus ? '\u25C9 Focus' : 'Focus'}</button>
            ${c.manual && html`<button class="btn btn-xs" style="color:var(--red);margin-left:4px" onClick=${() => removeClient(c.id)}>x</button>`}
          </div>`;
        })}
      </div>
    </div>
  </div>`;
}

// ── Control Page ──────────────────────────────────────────────
function ControlPage() {
  const { dispatch } = useContext(AppContext);
  useEffect(() => {
    apiFetch('/api/failover').then(d => dispatch({ type: 'SET_FAILOVER', data: d }));
  }, []);
  return html`<div class="control-layout">
    <${SignalFlowDiagram} />
    <${FailoverPanel} />
  </div>`;
}

function SignalFlowDiagram() {
  const { state, dispatch } = useContext(AppContext);
  const s = state.status;
  const midi = s.midi || {};
  const ir = s.input_redundancy || {};
  const t = s.traffic || {};
  const fo = s.failover || {};
  const dev = s.settings?.midi_device_status || 'disconnected';
  const devOk = dev === 'connected';
  const hostOk = s.health_score >= 50;
  const hasClients = s.client_count > 0;

  // Segment health
  const seg1Health = devOk && hostOk ? 'active' : devOk || hostOk ? 'warn' : 'err';
  const seg2Health = hostOk && hasClients ? 'active' : hostOk ? 'warn' : s.health_score > 0 ? 'warn' : '';
  const seg3Health = hasClients ? 'active' : '';

  const hMap = { active: 'ok', disconnected: 'err', error: 'err', reconnecting: 'warn', unknown: 'off' };
  const hasFocus = s.focus_holder != null;
  const focusClient = hasFocus ? state.clients.find(c => c.id === s.focus_holder) : null;

  return html`<div class="sf-diagram">
    <div class="sf-header">
      <span class="card-header-icon">${ICO.sliders()}</span>
      Signal Flow
      <div class="card-header-right">
        <button class="btn btn-sm" onClick=${() => dispatch({ type: 'SNIFFER_OPEN' })}>${ICO.search()} Sniffer</button>
      </div>
    </div>
    <div class="sf-body">
      <div class="sf-stages">

        <!-- Controllers Stage -->
        <div class="sf-stage">
          <div class="sf-stage-label">Controllers</div>
          <div class="sf-stage-nodes">
            <div class="sf-node" data-health=${hMap[ir.primary_health] || (devOk ? 'ok' : 'err')}>
              <span class="sf-node-dot" data-s=${hMap[ir.primary_health] || (devOk ? 'ok' : 'err')} />
              <div class="sf-node-info">
                <span class="sf-node-name">${ir.primary_device || 'Primary'}</span>
                <span class="sf-node-meta">${devOk ? fmtRate(midi.messages_per_sec || 0) + '/s' : ir.primary_health || 'disconnected'}</span>
              </div>
              ${ir.active_input === 0 && html`<span class="sf-node-badge" data-role="active">Active</span>`}
            </div>
            <div class="sf-node" data-health=${hMap[ir.secondary_health] || 'off'}>
              <span class="sf-node-dot" data-s=${hMap[ir.secondary_health] || 'off'} />
              <div class="sf-node-info">
                <span class="sf-node-name">${ir.secondary_device || 'Secondary'}</span>
                <span class="sf-node-meta">${ir.secondary_health || 'standby'}</span>
              </div>
              ${ir.active_input === 1 && html`<span class="sf-node-badge" data-role="active">Active</span>`}
            </div>
          </div>
        </div>

        <!-- Segment 1: Controllers → Hosts -->
        <div class="sf-segment">
          <span class="sf-seg-label ${seg1Health === 'active' ? 'active' : ''}">${fmtRate(midi.messages_per_sec || 0)}/s</span>
          <div class="sf-seg-line ${seg1Health}"><span class="sf-seg-arrow" /></div>
          <span class="sf-seg-protocol">MIDI</span>
        </div>

        <!-- Hosts Stage -->
        <div class="sf-stage">
          <div class="sf-stage-label">Hosts</div>
          <div class="sf-stage-nodes">
            ${state.hosts.length > 0 ? state.hosts.map(h => html`
              <div class="sf-node" data-health=${h.heartbeat_ok ? 'ok' : 'err'} key=${h.id}>
                <span class="sf-node-dot" data-s=${h.heartbeat_ok ? 'ok' : 'err'} />
                <div class="sf-node-info">
                  <span class="sf-node-name">${h.name || h.ip}</span>
                  <span class="sf-node-meta">${fmtUp(h.uptime_seconds)}</span>
                </div>
                <span class="sf-node-badge" data-role=${h.role}>${h.role}</span>
              </div>
            `) : html`
              <div class="sf-node" data-health=${hostOk ? 'ok' : 'warn'}>
                <span class="sf-node-dot" data-s=${hostOk ? 'ok' : 'warn'} />
                <div class="sf-node-info">
                  <span class="sf-node-name">${fo.active_host || 'primary'}</span>
                  <span class="sf-node-meta">Health ${s.health_score}/100</span>
                </div>
                <span class="sf-node-badge" data-role="primary">Active</span>
              </div>
              <div class="sf-node" data-health=${fo.standby_healthy ? 'ok' : 'off'}>
                <span class="sf-node-dot" data-s=${fo.standby_healthy ? 'ok' : 'off'} />
                <div class="sf-node-info">
                  <span class="sf-node-name">Standby</span>
                  <span class="sf-node-meta">${fo.standby_healthy ? 'Ready' : 'Unavailable'}</span>
                </div>
                <span class="sf-node-badge" data-role="standby">Standby</span>
              </div>
            `}
          </div>
        </div>

        <!-- Segment 2: Hosts → Network -->
        <div class="sf-segment">
          <span class="sf-seg-label ${seg2Health === 'active' ? 'active' : ''}">${fmtRate(t.midi_out_per_sec || 0)}/s</span>
          <div class="sf-seg-line ${seg2Health}"><span class="sf-seg-arrow" /></div>
          <span class="sf-seg-protocol">UDP</span>
        </div>

        <!-- Network Stage -->
        <div class="sf-stage">
          <div class="sf-stage-label">Network</div>
          <div class="sf-stage-nodes">
            <div class="sf-node" data-health=${hasClients ? 'ok' : hostOk ? 'warn' : 'off'}>
              <span class="sf-node-dot" data-s=${hasClients ? 'ok' : hostOk ? 'warn' : 'off'} />
              <div class="sf-node-info">
                <span class="sf-node-name">Multicast</span>
                <span class="sf-node-meta">${fmtRate((t.midi_in_per_sec||0) + (t.midi_out_per_sec||0) + (t.osc_per_sec||0))}/s total</span>
              </div>
            </div>
            <div class="sf-node" data-health=${(t.ws_connections || 0) > 0 ? 'ok' : 'off'}>
              <span class="sf-node-dot" data-s=${(t.ws_connections || 0) > 0 ? 'ok' : 'off'} />
              <div class="sf-node-info">
                <span class="sf-node-name">WebSocket</span>
                <span class="sf-node-meta">${t.ws_connections || 0} conn</span>
              </div>
            </div>
          </div>
        </div>

        <!-- Segment 3: Network → Clients -->
        <div class="sf-segment">
          <span class="sf-seg-label ${seg3Health === 'active' ? 'active' : ''}">${s.client_count} rx</span>
          <div class="sf-seg-line ${seg3Health}"><span class="sf-seg-arrow" /></div>
          <span class="sf-seg-protocol">Multicast</span>
        </div>

        <!-- Clients Stage -->
        <div class="sf-stage">
          <div class="sf-stage-label">Clients (${state.clients.length})</div>
          <div class="sf-stage-nodes" style="max-height:140px;overflow-y:auto">
            ${state.clients.length === 0 && html`
              <div class="sf-node" data-health="off">
                <span class="sf-node-dot" data-s="off" />
                <div class="sf-node-info">
                  <span class="sf-node-name" style="color:var(--text-3)">No clients</span>
                  <span class="sf-node-meta">Waiting...</span>
                </div>
              </div>
            `}
            ${state.clients.slice(0, 5).map(c => {
              const ch = c.packet_loss_percent > 1 ? 'err' : c.packet_loss_percent > 0.1 ? 'warn' : 'ok';
              const isFocus = s.focus_holder === c.id;
              return html`<div class="sf-node" data-health=${ch} key=${c.id}>
                <span class="sf-node-dot" data-s=${ch} />
                <div class="sf-node-info">
                  <span class="sf-node-name">${c.hostname}</span>
                  <span class="sf-node-meta">${c.latency_ms?.toFixed(1) || '—'}ms · ${c.packet_loss_percent?.toFixed(2) || '0'}%</span>
                </div>
                ${isFocus && html`<span class="sf-node-badge" data-role="focus">Focus</span>`}
              </div>`;
            })}
            ${state.clients.length > 5 && html`<div style="font-size:10px;color:var(--text-3);text-align:center;padding:4px">+${state.clients.length - 5} more</div>`}
          </div>
        </div>
      </div>

      <!-- Feedback Path -->
      <div class="sf-feedback">
        <span class="sf-feedback-arrow ${hasFocus ? 'active' : ''}" />
        <div class="sf-feedback-line ${hasFocus ? 'active' : ''}" />
        <span class="sf-feedback-label ${hasFocus ? 'active' : ''}">${hasFocus ? `Feedback: ${focusClient?.hostname || 'Client #' + s.focus_holder}` : 'No feedback focus'}</span>
        <div class="sf-feedback-line ${hasFocus ? 'active' : ''}" />
      </div>
    </div>
  </div>`;
}

function FailoverPanel() {
  const { state, dispatch } = useContext(AppContext);
  const fo = state.failoverDetail;
  const s = state.status;
  if (!fo) return html`<div class="card"><div class="card-header">Failover</div><div class="card-body"><div class="empty-state">Loading...</div></div></div>`;
  const doSwitch = () => {
    const go = async () => {
      const r = await apiFetch('/api/failover/switch', { method: 'POST' });
      if (r.success) { dispatch({ type: 'SET_FAILOVER', data: { ...fo, active_host: r.active_host, failover_count: r.failover_count } }); dispatch({ type: 'ADD_TOAST', toast: mkToast('success', `Switched to ${r.active_host}`) }); }
    };
    if (fo.confirmation_mode === 'confirm') {
      dispatch({ type: 'MODAL', modal: { title: 'Confirm Failover', message: `Switch from "${s.failover?.active_host}" to "${s.failover?.active_host === 'primary' ? 'standby' : 'primary'}"? MIDI output will briefly interrupt.`, onConfirm: go, ok: 'Switch', cls: 'btn-danger' } });
    } else go();
  };
  const toggleAuto = () => {
    const nv = !fo.auto_enabled;
    const go = async () => {
      const r = await apiFetch('/api/failover/auto', { method: 'PUT', body: JSON.stringify({ enabled: nv }) });
      if (r.success) { dispatch({ type: 'SET_FAILOVER', data: { ...fo, auto_enabled: r.auto_enabled } }); dispatch({ type: 'ADD_TOAST', toast: mkToast('info', `Auto-failover ${nv ? 'enabled' : 'disabled'}`) }); }
    };
    if (!nv) dispatch({ type: 'MODAL', modal: { title: 'Disable Auto-Failover?', message: 'Manual intervention will be required if the active host fails.', onConfirm: go, ok: 'Disable', cls: 'btn-danger' } });
    else go();
  };
  return html`<div class="card" style="flex-shrink:0">
    <div class="card-header">Failover Control</div>
    <div class="card-body" style="padding:12px 20px">
      <div class="flex items-center gap-md" style="flex-wrap:wrap">
        <div style="flex:1;min-width:120px">
          <div style="font-size:10px;color:var(--text-3);text-transform:uppercase;letter-spacing:.04em;font-weight:600">Active</div>
          <div style="font-size:18px;font-weight:700;font-family:var(--mono);color:var(--green)">${(fo.active_host || 'primary').toUpperCase()}</div>
        </div>
        <button class="btn btn-danger" onClick=${doSwitch}>Switch Host</button>
        <div style="display:flex;align-items:center;gap:8px;padding-left:12px;border-left:0.5px solid var(--border)">
          <span style="font-size:12px;font-weight:500">Auto</span>
          <button class="toggle ${fo.auto_enabled ? 'on' : ''}" onClick=${toggleAuto} />
        </div>
        <div style="display:flex;align-items:center;gap:6px;padding-left:12px;border-left:0.5px solid var(--border)">
          <span style="font-size:12px;color:var(--text-3)">Standby</span>
          <span class="status-dot" data-status=${s.failover?.standby_healthy ? 'ok' : 'error'} />
        </div>
        ${fo.history?.length > 0 && html`
          <div style="display:flex;align-items:center;gap:8px;padding-left:12px;border-left:0.5px solid var(--border);font-size:11px;color:var(--text-3)">
            ${fo.history.slice(-2).reverse().map((ev,i) => html`<span class="mono" key=${i}>${new Date(ev.timestamp*1000).toLocaleTimeString()} ${ev.from_host}→${ev.to_host}</span>`)}
          </div>
        `}
      </div>
    </div>
  </div>`;
}

// ── Settings Page ─────────────────────────────────────────────
function SettingsPage() {
  const { state, dispatch } = useContext(AppContext);
  useEffect(() => {
    apiFetch('/api/settings').then(d => dispatch({ type: 'SET_SETTINGS', data: d }));
    apiFetch('/api/settings/presets').then(d => dispatch({ type: 'SET_PRESETS', data: d.presets }));
    apiFetch('/api/devices').then(d => dispatch({ type: 'SET_DEVICES', data: d.devices }));
  }, []);
  return html`<div class="page-scroll">
    <div class="page-grid">
      <${DeviceSettings} />
      <${OscSettings} />
      <div class="card-wide"><${FailoverSettingsPanel} /></div>
      <div class="card-wide"><${PresetGrid} /></div>
    </div>
  </div>`;
}

function DeviceSettings() {
  const { state, dispatch } = useContext(AppContext);
  const s = state.settings?.midi_device;
  const [activeDid, setActiveDid] = useState('');
  const [backupDid, setBackupDid] = useState('');
  useEffect(() => { if (s?.active_device) setActiveDid(s.active_device); }, [s?.active_device]);
  useEffect(() => { setBackupDid(s?.backup_device || ''); }, [s?.backup_device]);
  const devices = state.devices || [];
  const activity = state.deviceActivity || {};
  const identifying = state.identifyActive || {};

  // Connect to device-activity WebSocket when settings page is open
  useEffect(() => {
    if (state.page !== 'settings') return;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/ws/device-activity`);
    ws.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        if (msg.type === 'snapshot') dispatch({ type: 'DEVICE_ACTIVITY_SNAPSHOT', data: msg.activity });
        else if (msg.type === 'update') dispatch({ type: 'DEVICE_ACTIVITY_UPDATE', data: msg.data });
      } catch {}
    };
    return () => ws.close();
  }, [state.page]);

  const saveRole = (role, deviceId) => {
    const clearing = role === 'backup' && !deviceId;
    const label = role === 'active' ? 'Active' : 'Backup';
    const msg = clearing ? 'Clear backup device assignment?' : `Assign "${deviceId}" as ${label}?`;
    dispatch({ type: 'MODAL', modal: { title: `${clearing ? 'Clear' : 'Change'} ${label} Device`, message: msg,
      onConfirm: async () => {
        const r = await apiFetch('/api/settings/midi-device', { method: 'PUT', body: JSON.stringify({ device_id: deviceId, role }) });
        dispatch({ type: 'ADD_TOAST', toast: mkToast(r.success ? 'success' : 'error', r.success ? (clearing ? 'Backup cleared' : `${label} device changed`) : (r.error || 'Failed')) });
        if (r.success) apiFetch('/api/settings').then(d => dispatch({ type: 'SET_SETTINGS', data: d }));
      }, ok: clearing ? 'Clear' : 'Apply',
    }});
  };

  const doIdentify = async (deviceId) => {
    dispatch({ type: 'IDENTIFY_START', deviceId });
    const r = await apiFetch(`/api/devices/${encodeURIComponent(deviceId)}/identify`, { method: 'POST' });
    if (r.success) {
      dispatch({ type: 'ADD_TOAST', toast: mkToast('info', 'Identifying device...') });
      setTimeout(() => dispatch({ type: 'IDENTIFY_END', deviceId }), r.duration_ms || 3000);
    } else {
      dispatch({ type: 'IDENTIFY_END', deviceId });
      dispatch({ type: 'ADD_TOAST', toast: mkToast('error', r.error || 'Identify failed') });
    }
  };

  const isActive = (deviceId) => {
    const a = activity[deviceId];
    return a && (Date.now() - a.last_activity_ms) < 2000;
  };

  return html`<div class="card">
    <div class="card-header">MIDI Controllers</div>
    <div class="card-body">
      <div class="form-group"><label class="form-label">Active Controller</label>
        <div class="form-row">
          <select value=${activeDid} onChange=${(e) => setActiveDid(e.target.value)} style="flex:1">
            <option value="">Select...</option>
            ${devices.filter(d => d.id !== backupDid).map(d => html`<option value=${d.id} key=${d.id}>${d.name}${d.connected ? '' : ' (offline)'}</option>`)}
          </select>
          <button class="btn btn-sm btn-accent" onClick=${() => saveRole('active', activeDid)} disabled=${!activeDid}>Apply</button>
        </div>
        <div class="flex items-center gap-sm mt-sm">
          <span class="status-dot" data-status=${s?.status === 'connected' ? 'ok' : s?.status === 'switching' ? 'warn' : 'disconnected'} />
          <span style="font-size:12px;color:var(--text-2)">${s?.status || 'disconnected'}</span>
        </div>
      </div>
      <div class="form-group" style="margin-top:16px"><label class="form-label">Backup Controller</label>
        <div class="form-row">
          <select value=${backupDid} onChange=${(e) => setBackupDid(e.target.value)} style="flex:1">
            <option value="">None</option>
            ${devices.filter(d => d.id !== activeDid).map(d => html`<option value=${d.id} key=${d.id}>${d.name}${d.connected ? '' : ' (offline)'}</option>`)}
          </select>
          <button class="btn btn-sm btn-accent" onClick=${() => saveRole('backup', backupDid)}>Apply</button>
        </div>
        ${backupDid && html`<div class="flex items-center gap-sm mt-sm">
          <span class="status-dot" data-status=${devices.find(d => d.id === backupDid)?.connected ? 'ok' : 'disconnected'} />
          <span style="font-size:12px;color:var(--text-2)">${devices.find(d => d.id === backupDid)?.connected ? 'standby' : 'disconnected'}</span>
        </div>`}
        ${!backupDid && html`<div style="font-size:11px;color:var(--text-3);margin-top:6px">No backup — input redundancy disabled</div>`}
      </div>
      ${devices.length > 0 && html`
        <div class="ctrl-section-label" style="margin-top:20px">Identify Devices</div>
        <div class="device-id-list">
          ${devices.map(d => {
            const act = isActive(d.id);
            const lastMsg = activity[d.id]?.last_message;
            const isId = identifying[d.id];
            const role = d.id === activeDid ? 'active' : d.id === backupDid ? 'backup' : null;
            return html`<div class="device-id-item ${act ? 'active' : ''}" key=${d.id}>
              <div class="device-id-dot ${act ? 'active' : ''}" />
              <div class="device-id-info">
                <div class="device-id-name">
                  ${d.name}
                  ${role && html`<span class="device-id-role" data-role=${role}>${role}</span>`}
                </div>
                <div class="device-id-meta">
                  ${d.manufacturer}${d.connected ? '' : ' (offline)'}${act && lastMsg ? html` · <span style="color:var(--green)">${lastMsg}</span>` : ''}
                </div>
              </div>
              <button class="btn btn-sm ${isId ? 'btn-identify-active' : ''}" onClick=${() => doIdentify(d.id)} disabled=${isId || !d.connected}>
                ${isId ? 'Flashing...' : 'Identify'}
              </button>
            </div>`;
          })}
        </div>
        <div style="font-size:10px;color:var(--text-3);margin-top:8px">Move a fader or press a key to see which device responds. Use Identify to flash the controller's LEDs.</div>
      `}
    </div>
  </div>`;
}

function OscSettings() {
  const { state, dispatch } = useContext(AppContext);
  const osc = state.settings?.osc;
  const [port, setPort] = useState('');
  useEffect(() => { if (osc?.listen_port) setPort(String(osc.listen_port)); }, [osc?.listen_port]);
  const save = async () => {
    const p = parseInt(port, 10);
    if (!p || p < 1024 || p > 65535) { dispatch({ type: 'ADD_TOAST', toast: mkToast('error', 'Port must be 1024-65535') }); return; }
    const r = await apiFetch('/api/settings/osc-port', { method: 'PUT', body: JSON.stringify({ port: p }) });
    dispatch({ type: 'ADD_TOAST', toast: mkToast(r.success ? 'success' : 'error', r.success ? `OSC port set to ${p}` : (r.error || 'Failed')) });
    if (r.success) apiFetch('/api/settings').then(d => dispatch({ type: 'SET_SETTINGS', data: d }));
  };
  return html`<div class="card">
    <div class="card-header">OSC Settings</div>
    <div class="card-body">
      <div class="form-group"><label class="form-label">Listen Port</label>
        <div class="form-row"><input type="number" value=${port} onInput=${(e) => setPort(e.target.value)} min="1024" max="65535" style="width:100px" /><button class="btn btn-sm btn-accent" onClick=${save}>Apply</button></div>
      </div>
      <div class="flex items-center gap-sm mt-sm">
        <span class="status-dot" data-status=${osc?.status === 'listening' ? 'ok' : 'error'} />
        <span style="font-size:12px;color:var(--text-2)">${osc?.status || 'stopped'}</span>
      </div>
    </div>
  </div>`;
}

function FailoverSettingsPanel() {
  const { state, dispatch } = useContext(AppContext);
  const fo = state.settings?.failover;
  const [cfg, setCfg] = useState(null);
  const [warns, setWarns] = useState([]);
  const [cap, setCap] = useState(null);
  useEffect(() => { if (fo && !cfg) setCfg(JSON.parse(JSON.stringify(fo))); }, [fo]);
  if (!cfg) return html`<div class="card card-wide"><div class="card-header">Failover Settings</div><div class="card-body"><div class="empty-state">Loading...</div></div></div>`;
  const u = (path, v) => { const n = JSON.parse(JSON.stringify(cfg)); const k = path.split('.'); let o = n; for (let i = 0; i < k.length - 1; i++) o = o[k[i]]; o[k[k.length-1]] = v; setCfg(n); };
  const save = async () => {
    const r = await apiFetch('/api/settings/failover', { method: 'PUT', body: JSON.stringify(cfg) });
    if (r.success) { dispatch({ type: 'ADD_TOAST', toast: mkToast('success', 'Settings saved') }); setWarns(r.warnings || []); apiFetch('/api/settings').then(d => dispatch({ type: 'SET_SETTINGS', data: d })); }
    else dispatch({ type: 'ADD_TOAST', toast: mkToast('error', r.error || 'Failed') });
  };
  useEffect(() => {
    if (cap !== 'midi') return;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/ws/traffic`);
    const t = setTimeout(() => { ws.close(); setCap(null); dispatch({ type: 'ADD_TOAST', toast: mkToast('warning', 'Capture timed out') }); }, 10000);
    ws.onmessage = (e) => { try { const d = JSON.parse(e.data); if (d.ch === 'midi' && d.msg) { const m = d.msg.match(/ch=(\d+)\s+note=(\d+)/i); if (m) { u('triggers.midi.channel', +m[1]); u('triggers.midi.note', +m[2]); ws.close(); clearTimeout(t); setCap(null); dispatch({ type: 'ADD_TOAST', toast: mkToast('success', `ch=${m[1]} note=${m[2]}`) }); } } } catch {} };
    return () => { clearTimeout(t); ws.close(); };
  }, [cap]);
  return html`<div class="card card-wide">
    <div class="card-header">Failover Settings<div class="card-header-right"><button class="btn btn-sm btn-accent" onClick=${save}>Save</button></div></div>
    <div class="card-body-flush">
      <div class="form-section">
        <div class="form-section-title">Core</div>
        <div class="flex gap-md" style="flex-wrap:wrap">
          <div class="form-group" style="flex:1;min-width:140px"><label class="form-label">Switch-back</label>
            <select value=${cfg.switch_back_policy} onChange=${(e) => u('switch_back_policy', e.target.value)}><option value="manual">Manual</option><option value="auto">Auto</option></select></div>
          <div class="form-group" style="flex:1;min-width:140px"><label class="form-label">Lockout (s)</label>
            <input type="number" value=${cfg.lockout_seconds} onInput=${(e) => u('lockout_seconds', +e.target.value||0)} min="0" max="300" /></div>
          <div class="form-group" style="flex:1;min-width:140px"><label class="form-label">Confirmation</label>
            <select value=${cfg.confirmation_mode} onChange=${(e) => u('confirmation_mode', e.target.value)}><option value="immediate">Immediate</option><option value="confirm">Confirm</option></select></div>
        </div>
      </div>
      <div class="form-section">
        <div class="form-section-title">Heartbeat</div>
        <div class="flex gap-md" style="flex-wrap:wrap">
          <div class="form-group" style="flex:1;min-width:120px"><label class="form-label">Interval (ms)</label><input type="number" value=${cfg.heartbeat?.interval_ms} onInput=${(e) => u('heartbeat.interval_ms', +e.target.value||1)} min="1" max="1000" /></div>
          <div class="form-group" style="flex:1;min-width:120px"><label class="form-label">Miss Threshold</label><input type="number" value=${cfg.heartbeat?.miss_threshold} onInput=${(e) => u('heartbeat.miss_threshold', +e.target.value||1)} min="1" max="20" /></div>
          <div class="form-group" style="flex:1;min-width:120px"><label class="form-label">Detection</label><span class="mono" style="font-size:14px;padding-top:6px;color:var(--accent)">${(cfg.heartbeat?.interval_ms||3)*(cfg.heartbeat?.miss_threshold||3)}ms</span></div>
        </div>
      </div>
      <div class="form-section">
        <div class="form-section-title">MIDI Trigger</div>
        <div class="flex items-center justify-between mb-sm"><span style="font-size:12px">Enable</span><button class="toggle ${cfg.triggers?.midi?.enabled?'on':''}" onClick=${()=>u('triggers.midi.enabled',!cfg.triggers?.midi?.enabled)} /></div>
        ${cfg.triggers?.midi?.enabled && html`<div class="flex gap-md" style="flex-wrap:wrap">
          <div class="form-group" style="min-width:70px"><label class="form-label">Channel</label><input type="number" value=${cfg.triggers.midi.channel} onInput=${(e)=>u('triggers.midi.channel',+e.target.value||1)} min="1" max="16" style="width:70px" /></div>
          <div class="form-group" style="min-width:70px"><label class="form-label">Note</label><input type="number" value=${cfg.triggers.midi.note} onInput=${(e)=>u('triggers.midi.note',+e.target.value||0)} min="0" max="127" style="width:70px" /></div>
          <div class="form-group" style="min-width:70px"><label class="form-label">Velocity</label><input type="number" value=${cfg.triggers.midi.velocity_threshold} onInput=${(e)=>u('triggers.midi.velocity_threshold',+e.target.value||0)} min="0" max="127" style="width:70px" /></div>
          <div class="form-group" style="align-self:flex-end"><button class="btn btn-sm ${cap==='midi'?'btn-warn':''}" onClick=${()=>setCap(cap==='midi'?null:'midi')}>${cap==='midi'?'Listening...':'Capture'}</button></div>
        </div>`}
      </div>
      <div class="form-section">
        <div class="form-section-title">OSC Trigger</div>
        <div class="flex items-center justify-between mb-sm"><span style="font-size:12px">Enable</span><button class="toggle ${cfg.triggers?.osc?.enabled?'on':''}" onClick=${()=>u('triggers.osc.enabled',!cfg.triggers?.osc?.enabled)} /></div>
        ${cfg.triggers?.osc?.enabled && html`<div class="flex gap-md" style="flex-wrap:wrap">
          <div class="form-group" style="flex:1;min-width:100px"><label class="form-label">Port</label><input type="number" value=${cfg.triggers.osc.listen_port} onInput=${(e)=>u('triggers.osc.listen_port',+e.target.value||8000)} min="1024" max="65535" /></div>
          <div class="form-group" style="flex:2;min-width:200px"><label class="form-label">Address</label><input type="text" value=${cfg.triggers.osc.address} onInput=${(e)=>u('triggers.osc.address',e.target.value)} /></div>
        </div>`}
      </div>
      ${warns.length > 0 && html`<div class="form-section" style="background:var(--orange-dim)">${warns.map((w,i)=>html`<div class="form-warn" key=${i}>${w}</div>`)}</div>`}
    </div>
  </div>`;
}

function PresetGrid() {
  const { state, dispatch } = useContext(AppContext);
  const active = state.settings?.active_preset;
  const apply = async (id) => {
    const r = await apiFetch('/api/settings/preset', { method: 'POST', body: JSON.stringify({ preset: id }) });
    if (r.success) { dispatch({ type: 'ADD_TOAST', toast: mkToast('success', `"${r.name}" applied`) }); apiFetch('/api/settings').then(d => dispatch({ type: 'SET_SETTINGS', data: d })); }
    else dispatch({ type: 'ADD_TOAST', toast: mkToast('error', r.error || 'Failed') });
  };
  return html`<div class="card card-wide">
    <div class="card-header">Presets</div>
    <div class="card-body">
      <div class="preset-grid">
        ${(state.presets||[]).map(p => html`<div class="preset-card ${active===p.id?'active':''}" key=${p.id} onClick=${()=>apply(p.id)}>
          <div class="preset-name">${p.name}</div><div class="preset-desc">${p.description}</div>
        </div>`)}
      </div>
      ${(!state.presets||!state.presets.length) && html`<div class="empty-state">No presets</div>`}
    </div>
  </div>`;
}

// ── Help Page ─────────────────────────────────────────────────
function HelpPage() {
  const { dispatch } = useContext(AppContext);
  const cmd = (text) => html`<div class="cmd-block"><span class="cmd-text">${text}</span><button class="cmd-copy" onClick=${(e)=>{copyText(text,dispatch);e.target.textContent='OK';setTimeout(()=>e.target.textContent='COPY',1500)}}>COPY</button></div>`;
  return html`<div class="page-scroll">
    <div class="help-section">
      <div class="help-title">Quick Start</div>
      <div class="help-subtitle">macOS</div>${cmd('curl -fsSL https://raw.githubusercontent.com/yourorg/midinet/main/scripts/client-install-macos.sh | bash')}
      <div class="help-subtitle">Linux</div>${cmd('curl -fsSL https://raw.githubusercontent.com/yourorg/midinet/main/scripts/client-install-linux.sh | bash')}
      <div class="help-subtitle">Windows</div>${cmd('irm https://raw.githubusercontent.com/yourorg/midinet/main/scripts/client-install-windows.ps1 | iex')}
    </div>
    <div class="help-section">
      <div class="help-title">API Reference</div>
      <table class="tbl ref-tbl"><thead><tr><th>Method</th><th>Endpoint</th><th>Description</th></tr></thead><tbody>
        <tr><td>GET</td><td class="mono">/api/status</td><td>System status</td></tr>
        <tr><td>GET</td><td class="mono">/api/hosts</td><td>Host list</td></tr>
        <tr><td>GET</td><td class="mono">/api/clients</td><td>Connected clients</td></tr>
        <tr><td>GET</td><td class="mono">/api/devices</td><td>MIDI devices</td></tr>
        <tr><td>GET/PUT</td><td class="mono">/api/pipeline</td><td>Pipeline config</td></tr>
        <tr><td>GET</td><td class="mono">/api/failover</td><td>Failover state</td></tr>
        <tr><td>POST</td><td class="mono">/api/failover/switch</td><td>Trigger switch</td></tr>
        <tr><td>PUT</td><td class="mono">/api/failover/auto</td><td>Auto-failover toggle</td></tr>
        <tr><td>GET</td><td class="mono">/api/input-redundancy</td><td>Input redundancy</td></tr>
        <tr><td>GET</td><td class="mono">/api/settings</td><td>Full settings</td></tr>
        <tr><td>PUT</td><td class="mono">/api/settings/midi-device</td><td>Assign device (role: active/backup)</td></tr>
        <tr><td>PUT</td><td class="mono">/api/settings/failover</td><td>Update failover config</td></tr>
        <tr><td>GET</td><td class="mono">/api/alerts</td><td>Active alerts</td></tr>
      </tbody></table>
    </div>
    <div class="help-section">
      <div class="help-title">Network</div>
      <table class="tbl ref-tbl"><thead><tr><th>Port</th><th>Protocol</th><th>Use</th></tr></thead><tbody>
        <tr><td class="mono">5004</td><td>UDP Multicast</td><td>MIDI data</td></tr>
        <tr><td class="mono">5005</td><td>UDP Multicast</td><td>Heartbeat</td></tr>
        <tr><td class="mono">5006</td><td>UDP Unicast</td><td>Control</td></tr>
        <tr><td class="mono">8080</td><td>HTTP</td><td>Admin panel + API</td></tr>
        <tr><td class="mono">8000</td><td>UDP</td><td>OSC (configurable)</td></tr>
        <tr><td class="mono">5353</td><td>mDNS</td><td>Discovery</td></tr>
      </tbody></table>
    </div>
    <div class="help-section">
      <div class="help-title">WebSocket Streams</div>
      <table class="tbl ref-tbl"><thead><tr><th>Endpoint</th><th>Rate</th><th>Data</th></tr></thead><tbody>
        <tr><td class="mono">/ws/status</td><td>1s</td><td>System status + metrics</td></tr>
        <tr><td class="mono">/ws/traffic</td><td>Live</td><td>Traffic sniffer</td></tr>
        <tr><td class="mono">/ws/alerts</td><td>5s</td><td>Alert deltas</td></tr>
        <tr><td class="mono">/ws/midi</td><td>Live</td><td>Raw MIDI stream</td></tr>
      </tbody></table>
    </div>
    <div class="help-section">
      <div class="help-title">Keyboard Shortcuts</div>
      <table class="tbl ref-tbl"><thead><tr><th>Key</th><th>Action</th></tr></thead><tbody>
        <tr><td class="mono">1</td><td>Overview</td></tr>
        <tr><td class="mono">2</td><td>Control</td></tr>
        <tr><td class="mono">3</td><td>Settings</td></tr>
        <tr><td class="mono">4</td><td>Help</td></tr>
        <tr><td class="mono">Esc</td><td>Close overlay</td></tr>
      </tbody></table>
    </div>
  </div>`;
}

// ── App Root ──────────────────────────────────────────────────
function App() {
  const [state, dispatch] = useReducer(reducer, INIT);

  useEffect(() => {
    const go = () => dispatch({ type: 'SET_PAGE', page: window.location.hash.slice(1) || 'overview' });
    window.addEventListener('hashchange', go); go();
    return () => window.removeEventListener('hashchange', go);
  }, []);

  useEffect(() => {
    const onKey = (e) => {
      if (['INPUT','SELECT','TEXTAREA'].includes(e.target.tagName)) return;
      const pages = ['overview','control','settings','help'];
      if (e.key >= '1' && e.key <= '4') window.location.hash = '#' + pages[+e.key - 1];
      if (e.key === 'Escape') { dispatch({ type: 'MODAL_CLOSE' }); dispatch({ type: 'SNIFFER_CLOSE' }); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useWS('/ws/status', (d) => dispatch({ type: 'WS_STATUS', data: d }), (ok) => dispatch({ type: 'WS_OK', v: ok }));
  usePoll('/api/hosts', 5000, (d) => dispatch({ type: 'SET_HOSTS', data: d.hosts }));
  usePoll('/api/clients', 5000, (d) => dispatch({ type: 'SET_CLIENTS', data: d.clients }));
  usePoll('/api/alerts', 10000, (d) => dispatch({ type: 'SET_ALERTS', data: d.active_alerts }));

  useWarningPopups(state.alerts, state.mutedAlerts, dispatch);

  const ctx = useMemo(() => ({ state, dispatch }), [state]);

  return html`<${AppContext.Provider} value=${ctx}>
    <${Header} />
    <main class="main">
      ${state.page === 'overview' && html`<${OverviewPage} />`}
      ${state.page === 'control' && html`<${ControlPage} />`}
      ${state.page === 'settings' && html`<${SettingsPage} />`}
      ${state.page === 'help' && html`<${HelpPage} />`}
    </main>
    <${Footer} />
    <${ConfirmModal} />
    <${ToastContainer} />
    <${WarningPopupContainer} />
    <${SnifferDrawer} />
  <//>`;
}

render(html`<${App} />`, document.getElementById('app'));
