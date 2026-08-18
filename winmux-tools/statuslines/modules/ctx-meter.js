// winmux-tools statusline module: context-window usage meter.
// Extracted verbatim from the sabra statusline.js monolith (lines 54-76).
// Honors the legacy sabra.conf toggles SABRA_CTX / SABRA_CTX_STYLE.
// Contract: module.exports = (data, cfg) => string
module.exports = (data, cfg) => {
  if (cfg.SABRA_CTX === '0') return '';
  const cw = data.context_window || {};
  const used = cw.total_input_tokens || 0;
  const total = cw.context_window_size || 0;
  const pct = cw.used_percentage || 0;
  if (!(total > 0)) return '';

  const pctInt = Math.floor(parseFloat(pct) || 0);
  let ccolor;
  if (pctInt < 50) ccolor = '\x1b[32m';
  else if (pctInt < 80) ccolor = '\x1b[33m';
  else ccolor = '\x1b[31m';

  if ((cfg.SABRA_CTX_STYLE || 'wide') === 'compact') {
    return `${ccolor}${pctInt}%\x1b[0m`;
  }

  let filled = Math.floor(pctInt / 10);
  if (filled > 10) filled = 10;
  const bar = '▓'.repeat(filled) + '░'.repeat(10 - filled);
  const fmt = (n) => {
    if (n >= 1000000) {
      const t = Math.floor(n / 100000);
      return (t % 10 === 0) ? `${t / 10}M` : `${Math.floor(t / 10)}.${t % 10}M`;
    }
    return `${Math.floor(n / 1000)}k`;
  };
  return `${ccolor}[${bar}] ${pctInt}% ${fmt(used)}/${fmt(total)}\x1b[0m`;
};
