(() => {
  const $ = (id) => document.getElementById(id);
  const tauri = window.__TAURI__;
  const invoke = tauri?.core?.invoke || tauri?.invoke;
  const listen = tauri?.event?.listen;
  const getCurrentWindow = tauri?.window?.getCurrentWindow;

  let state = null;
  let settings = null;
  let samples = [];
  let saveTimer = null;

  function esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, c => ({ '&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;' }[c]));
  }

  async function init() {
    // Window controls
    $('btn-minimize')?.addEventListener('click', async () => {
      try { await getCurrentWindow?.().minimize(); } catch {}
    });
    $('btn-close')?.addEventListener('click', async () => {
      try { await getCurrentWindow?.().close(); } catch {}
    });

    // Actions
    $('btn-calibrate')?.addEventListener('click', async () => {
      const btn = $('btn-calibrate');
      const prev = btn?.textContent;
      try {
        if (btn) { btn.textContent = 'Calibrating…'; btn.disabled = true; }
        await invoke('calibrate');
        if (btn) { btn.textContent = 'Calibrated'; }
      } catch (e) {
        console.error(e);
        if (btn) { btn.textContent = 'Calibrate failed'; }
      } finally {
        setTimeout(() => {
          if (btn) { btn.textContent = prev || 'Calibrate'; btn.disabled = false; }
        }, 1500);
      }
    });
    $('btn-pause')?.addEventListener('click', async () => {
      const paused = !!state?.paused;
      try { await invoke('set_paused', { paused: !paused }); } catch (e) { console.error(e); }
    });

    // Settings inputs
    ['interval-sec','sensitivity','battery-pause','battery-threshold','autostart'].forEach(id => {
      $(id)?.addEventListener('change', scheduleSaveSettings);
      $(id)?.addEventListener('input', (e) => {
        if (e.target.type !== 'checkbox') scheduleSaveSettings();
      });
    });

    if (listen) {
      await listen('state', (event) => {
        state = event.payload;
        if (state?.settings) settings = state.settings;
        render();
      });
    }

    try {
      state = await invoke('get_state');
      if (state?.settings) settings = state.settings;
      render();
      syncSettingsInputs();
    } catch (e) {
      console.error('init failed', e);
    }
  }

  function scheduleSaveSettings() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(saveSettings, 400);
  }

  async function saveSettings() {
    const base = state?.settings || settings;
    if (!base) return;
    const next = { ...base };
    const intervalRaw = parseInt($('interval-sec')?.value, 10);
    next.intervalSec = Number.isFinite(intervalRaw) && intervalRaw > 0 ? intervalRaw : base.intervalSec;
    const sensRaw = parseFloat($('sensitivity')?.value);
    next.sensitivity = Number.isFinite(sensRaw) ? Math.max(0.1, Math.min(2.0, sensRaw)) : base.sensitivity;
    next.batteryPauseEnabled = !!$('battery-pause')?.checked;
    const thrRaw = parseInt($('battery-threshold')?.value, 10);
    next.batteryPauseThreshold = Number.isFinite(thrRaw) ? Math.max(1, Math.min(100, thrRaw)) : base.batteryPauseThreshold;
    next.autostart = !!$('autostart')?.checked;
    // preserve trims/baselines/paused from latest state (source of truth)
    next.trims = { ...(state?.settings?.trims || base.trims || {}) };
    next.baselineBrightness = state?.settings?.baselineBrightness ?? base.baselineBrightness;
    next.baselineAmbient = state?.settings?.baselineAmbient ?? base.baselineAmbient;
    next.paused = state?.paused ?? base.paused;
    settings = next;
    try { await invoke('update_settings', { settings: next }); } catch (e) { console.error(e); }
  }

  function syncSettingsInputs() {
    const s = state?.settings;
    if (!s) return;
    setIfNotFocused('interval-sec', s.intervalSec);
    setIfNotFocused('sensitivity', s.sensitivity);
    setCheckIfNotFocused('battery-pause', !!s.batteryPauseEnabled);
    setIfNotFocused('battery-threshold', s.batteryPauseThreshold);
    setCheckIfNotFocused('autostart', !!s.autostart);
  }

  function setIfNotFocused(id, val) {
    const el = $(id);
    if (!el || document.activeElement === el) return;
    if (val === undefined || val === null) return;
    if (String(el.value) !== String(val)) el.value = val;
  }
  function setCheckIfNotFocused(id, val) {
    const el = $(id);
    if (!el || document.activeElement === el) return;
    el.checked = !!val;
  }

  function render() {
    if (!state) return;
    renderAmbient(state.ambient);
    renderDisplays(state.displays || []);
    renderFooter();
    syncSettingsInputs();
  }

  function renderAmbient(ambient) {
    if (!ambient) return;
    const idx = ambient.index;
    const hasMeasurement = Number.isFinite(idx);
    $('ambient-value').textContent = hasMeasurement ? String(Math.round(idx)) : '--';
    const mode = String(ambient.mode || '--').toUpperCase();
    $('ambient-mode').textContent = mode;
    const modeTitle = mode === 'LOCKED'
      ? 'LOCKED: manual camera exposure is active; ambient index uses mean luma with clipping correction.'
      : mode === 'COARSE'
        ? 'COARSE: manual exposure lock unavailable; ambient index uses clipping-ratio weighting only.'
        : mode === 'UNAVAILABLE'
          ? 'UNAVAILABLE: no ambient sample available.'
          : 'Camera sampling mode.';
    $('ambient-mode').title = modeTitle;

    let status = ambient.status || '';
    if (!hasMeasurement && (status === '' || status === 'initializing')) {
      status = 'Waiting for first sample';
    }
    if (status === 'ok') {
      status = mode === 'LOCKED' ? 'Exposure locked' : (mode === 'COARSE' ? 'Coarse mode - exposure not locked' : 'No camera');
    }
    $('ambient-status').textContent = status || 'Waiting for first sample';
    $('ambient-status').title = $('ambient-status').textContent || 'Current ambient sensor/camera status.';

    if (hasMeasurement) {
      samples.push(idx);
      if (samples.length > 80) samples.shift();
    }
    drawSparkline();
  }

  function drawSparkline() {
    const c = $('sparkline');
    if (!c) return;
    const ctx = c.getContext('2d');
    const w = c.width, h = c.height;
    ctx.clearRect(0, 0, w, h);
    // baseline grid line
    ctx.strokeStyle = '#26282C';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, h - 0.5);
    ctx.lineTo(w, h - 0.5);
    ctx.stroke();
    if (samples.length < 2) return;
    ctx.strokeStyle = '#D6A756';
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let i = 0; i < samples.length; i++) {
      const x = (i / (samples.length - 1)) * (w - 2) + 1;
      const v = Math.max(0, Math.min(100, samples[i]));
      const y = h - 1 - (v / 100) * (h - 4);
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.stroke();
  }

  function renderDisplays(displays) {
    const container = $('displays-list');
    if (!container) return;
    container.innerHTML = '';
    displays.forEach(d => {
      const card = document.createElement('div');
      card.className = 'display-card';
      const method = (d.method || '').toUpperCase();
      const supported = !!d.supported;

      let bodyHtml = '';
      let badgeText = method || (supported ? 'WMI' : 'UNSUPPORTED');
      let badgeClass = 'badge';
      let badgeTitle = method === 'WMI'
        ? 'WMI: internal/laptop panel brightness control via Windows monitor brightness API.'
        : method === 'DDC'
          ? 'DDC/CI: external monitor brightness control via Display Data Channel.'
          : 'Unsupported: this display cannot be controlled automatically.';

      if (supported) {
        const b = (d.brightness !== null && d.brightness !== undefined) ? `${d.brightness}%` : '--';
        const trim = Number.isFinite(d.trim) ? d.trim : 0;
        const status = (d.status && d.status !== 'ok') ? d.status : (method === 'DDC' ? 'DDC ready' : 'WMI ready');
        bodyHtml = `
          <div class="display-main">
            <div class="brightness-value tabular" title="Current brightness level for this display.">${esc(b)}</div>
            <div class="trim-control">
              <label title="Per-display offset applied to the computed target brightness (-20 to +20).">TRIM</label>
              <div class="stepper" title="Adjust per-display trim offset.">
                <button data-action="trim-dec" data-id="${esc(d.id)}" aria-label="Decrease trim" title="Decrease trim by 1.">−</button>
                <span class="trim-val tabular" title="Current trim offset: ${trim > 0 ? '+' : ''}${trim}">${trim > 0 ? '+' : ''}${trim}</span>
                <button data-action="trim-inc" data-id="${esc(d.id)}" aria-label="Increase trim" title="Increase trim by 1.">+</button>
              </div>
            </div>
          </div>
          <div class="display-status" title="${esc(status)}">${esc(status)}</div>
        `;
      } else {
        badgeText = 'UNSUPPORTED';
        badgeClass = 'badge unsupported';
        badgeTitle = 'Unsupported: automatic brightness control is unavailable for this display.';
        const status = d.status || 'Unsupported (DDC/CI unavailable)';
        const hrMatch = status.match(/HRESULT\s+(0x[0-9A-Fa-f]+)/);
        const hr = hrMatch ? hrMatch[1] : null;
        const primary = 'Unsupported (DDC/CI unavailable)';
        bodyHtml = `
          <div class="display-status unsupported" title="${esc(status)}">
            <div class="status-primary" title="Automatic brightness control is unavailable for this display.">${esc(primary)}</div>
            <div class="hint-line" title="Many monitors ship with DDC/CI disabled; enable it in the monitor OSD if supported.">DDC/CI may be disabled in the monitor OSD.</div>
            ${hr ? `<details class="hresult"><summary title="Show technical failure details.">Details (HRESULT ${esc(hr)})</summary><div class="detail-body">${esc(status)}</div></details>` : `<details class="hresult"><summary title="Show technical failure details.">Details</summary><div class="detail-body">${esc(status)}</div></details>`}
          </div>
        `;
      }

      card.innerHTML = `
        <div class="display-header">
          <div class="display-name" title="${esc(d.name || d.id)}">${esc(d.name || d.id)}</div>
          <span class="${badgeClass}" title="${esc(badgeTitle)}">${esc(badgeText)}</span>
        </div>
        ${bodyHtml}
      `;
      container.appendChild(card);
    });

    // Trim handlers
    container.querySelectorAll('button[data-action="trim-dec"], button[data-action="trim-inc"]').forEach(btn => {
      btn.addEventListener('click', async () => {
        const id = btn.getAttribute('data-id');
        const action = btn.getAttribute('data-action');
        await adjustTrim(id, action === 'trim-inc' ? 1 : -1);
      });
    });
  }

  async function adjustTrim(displayId, delta) {
    const base = state?.settings || settings;
    if (!base) return;
    const s = { ...base };
    const trims = { ...(s.trims || {}) };
    const cur = Number.isFinite(trims[displayId]) ? trims[displayId] : 0;
    trims[displayId] = Math.max(-20, Math.min(20, cur + delta));
    s.trims = trims;
    try { await invoke('update_settings', { settings: s }); } catch (e) { console.error(e); }
  }

  function renderFooter() {
    const paused = !!state?.paused;
    const p = state?.power;
    const parts = [];
    if (paused) {
      if (p?.shouldPause) parts.push('paused (battery)');
      else parts.push('paused');
    } else {
      parts.push('active');
    }
    if (state?.targetBrightness != null) parts.push(`target ${Math.round(state.targetBrightness)}%`);
    $('footer-state').textContent = parts.join(' · ');
    const targetPart = state?.targetBrightness != null ? ` Target brightness: ${Math.round(state.targetBrightness)}%.` : '';
    $('footer-state').title = `Automation status.${targetPart}`;
    $('btn-pause').textContent = paused ? 'Resume' : 'Pause';
    $('btn-pause').title = paused ? 'Resume automatic brightness adjustments.' : 'Pause automatic brightness adjustments.';
  }

  init();
})();
