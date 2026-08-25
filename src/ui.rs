use maud::{DOCTYPE, Markup, PreEscaped, html};
use time::{OffsetDateTime, macros::format_description};

use crate::model::{CreateResult, DropDetail, DropRecord, Entry, EntryKind};

const CSS: &str = r#"
:root {
  color-scheme: dark;
  --void: #05070a;
  --void-2: #090d12;
  --shell-solid: #0c1218;
  --ink: #f0f7f5;
  --muted: #8fa2a2;
  --quiet: #5f7173;
  --line: rgba(163, 255, 226, .14);
  --line-hot: rgba(155, 255, 220, .43);
  --mint: #9bffdc;
  --cyan: #6ee7ff;
  --violet: #b5a3ff;
  --amber: #ffd08a;
  --danger: #ff9a9a;
  --radius: 22px;
  --mono: "SFMono-Regular", "Cascadia Code", "Roboto Mono", Consolas, monospace;
  --sans: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}
* { box-sizing: border-box; }
* { scrollbar-width: thin; scrollbar-color: var(--mint) var(--void); }
*::-webkit-scrollbar { width: 8px; height: 8px; }
*::-webkit-scrollbar-track { background: var(--void); }
*::-webkit-scrollbar-thumb { border: 2px solid var(--void); border-radius: 999px; background: var(--mint); }
*::-webkit-scrollbar-corner { background: var(--void); }
html { min-height: 100%; background: var(--void); scroll-behavior: smooth; }
body {
  min-height: 100vh; margin: 0; overflow-x: hidden; color: var(--ink);
  font-family: var(--sans); font-size: 15px; line-height: 1.55;
  background:
    radial-gradient(ellipse 70% 54% at 50% -15%, rgba(110,231,255,.12), transparent 70%),
    radial-gradient(ellipse 46% 60% at 104% 68%, rgba(181,163,255,.09), transparent 68%),
    radial-gradient(ellipse 40% 50% at -8% 82%, rgba(155,255,220,.08), transparent 67%),
    linear-gradient(180deg, #06090d 0%, #040609 100%);
  -webkit-font-smoothing: antialiased; text-rendering: optimizeLegibility;
}
body::before {
  content: ""; position: fixed; inset: 0; z-index: 0; pointer-events: none; opacity: .22;
  background-image:
    linear-gradient(rgba(151,255,224,.045) 1px, transparent 1px),
    linear-gradient(90deg, rgba(151,255,224,.045) 1px, transparent 1px);
  background-size: 56px 56px;
  mask-image: radial-gradient(ellipse at 50% 25%, #000 0, transparent 74%);
}
body::after {
  content: ""; position: fixed; inset: 0; z-index: 0; pointer-events: none; opacity: .25;
  background-image: url("data:image/svg+xml,%3Csvg viewBox='0 0 180 180' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='.13'/%3E%3C/svg%3E");
  mix-blend-mode: soft-light;
}
a { color: inherit; }
button, input { font: inherit; }
::selection { color: #05100d; background: var(--mint); }
:focus-visible { outline: 2px solid var(--mint); outline-offset: 4px; }
.skip { position: fixed; top: 10px; left: 10px; z-index: 20; transform: translateY(-160%); padding: 9px 13px; background: var(--mint); color: #06100e; border-radius: 9px; }
.skip:focus { transform: none; }
.ambient { position: fixed; inset: 0; pointer-events: none; z-index: 0; overflow: hidden; }
.orb { position: absolute; border: 1px solid rgba(155,255,220,.1); border-radius: 50%; filter: blur(.2px); }
.orb-a { width: 720px; height: 720px; top: -510px; left: calc(50% - 360px); box-shadow: inset 0 0 120px rgba(110,231,255,.07), 0 0 90px rgba(110,231,255,.04); }
.orb-b { width: 310px; height: 310px; right: -170px; top: 42%; border-color: rgba(181,163,255,.09); }
.beam { position: absolute; width: 1px; height: 62vh; top: 0; left: 50%; background: linear-gradient(180deg, rgba(155,255,220,.34), transparent); box-shadow: 0 0 22px rgba(155,255,220,.35); opacity: .42; }
.page { position: relative; z-index: 1; width: min(1120px, calc(100% - 40px)); margin: 0 auto; padding: 30px 0 64px; display: flex; flex-direction: column; min-height: 100vh; }
.mast { display: flex; align-items: center; justify-content: space-between; gap: 20px; min-height: 56px; flex: 0 0 auto; }
.identity { display: inline-flex; align-items: center; gap: 12px; text-decoration: none; }
.sigil { width: 32px; height: 32px; filter: drop-shadow(0 0 13px rgba(155,255,220,.18)); }
.wordmark { font-family: var(--mono); font-size: 12px; font-weight: 650; letter-spacing: .19em; text-transform: uppercase; }
.wordmark span { color: var(--mint); }
.link-state { display: inline-flex; align-items: center; gap: 8px; color: var(--muted); font-size: 12px; }
.pulse { width: 6px; height: 6px; background: var(--mint); border-radius: 50%; box-shadow: 0 0 0 5px rgba(155,255,220,.07), 0 0 16px rgba(155,255,220,.5); }
.landing { flex: 1 1 auto; display: grid; place-items: center; padding: 44px 0; }
.artifact { width: min(680px, 100%); position: relative; }
.frame { position: relative; }
.frame::before { content: ""; position: absolute; inset: -1px; border-radius: calc(var(--radius) + 1px); padding: 1px; background: linear-gradient(135deg, rgba(155,255,220,.64), rgba(110,231,255,.08) 36%, rgba(181,163,255,.24) 68%, rgba(255,141,184,.2)); -webkit-mask: linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0); -webkit-mask-composite: xor; mask-composite: exclude; pointer-events: none; }
.artifact-panel { position: relative; overflow: hidden; padding: clamp(28px, 7vw, 64px); border-radius: var(--radius); background: linear-gradient(150deg, rgba(15,23,30,.94), rgba(8,13,18,.88)); box-shadow: 0 30px 100px rgba(0,0,0,.46), inset 0 1px rgba(255,255,255,.035); backdrop-filter: blur(22px); }
.artifact-panel::after { content: ""; position: absolute; width: 280px; height: 280px; right: -160px; top: -160px; border: 1px solid rgba(155,255,220,.17); border-radius: 50%; box-shadow: 0 0 0 28px rgba(155,255,220,.016), 0 0 0 70px rgba(110,231,255,.012); pointer-events: none; }
.eyebrow { display: flex; align-items: center; gap: 10px; margin: 0 0 22px; color: var(--mint); font: 10px/1.4 var(--mono); letter-spacing: .14em; text-transform: uppercase; }
.eyebrow::before { content: ""; width: 24px; height: 1px; background: linear-gradient(90deg, var(--mint), transparent); box-shadow: 0 0 9px var(--mint); }
h1 { margin: 0; max-width: 570px; font-size: clamp(35px, 7vw, 66px); line-height: .99; letter-spacing: -.052em; font-weight: 590; }
h1 .spectral { color: transparent; background: linear-gradient(100deg, var(--mint), var(--cyan) 48%, var(--violet)); background-clip: text; -webkit-background-clip: text; }
.lede { max-width: 505px; margin: 22px 0 0; color: var(--muted); font-size: clamp(14px, 2.2vw, 17px); }
.redeem { margin-top: clamp(34px, 6vw, 48px); }
.code-label { display: flex; justify-content: space-between; align-items: flex-end; gap: 16px; margin-bottom: 14px; }
.code-label label { font: 10px/1 var(--mono); color: var(--muted); text-transform: uppercase; letter-spacing: .16em; }
.code-label span { color: var(--quiet); font: 10px/1 var(--mono); }
.code-field { display: grid; grid-template-columns: 1fr auto; gap: 12px; }
.code-input { width: 100%; min-width: 0; height: 68px; padding: 0 12px 0 27px; border: 1px solid var(--line); border-radius: 14px; color: var(--ink); caret-color: var(--mint); background: rgba(0,0,0,.23); font: 600 clamp(22px, 5vw, 32px)/1 var(--mono); letter-spacing: .49em; font-variant-numeric: tabular-nums; text-transform: uppercase; transition: border-color .2s, box-shadow .2s, background .2s; }
.code-input:hover { border-color: rgba(155,255,220,.25); }
.code-input:focus { border-color: var(--line-hot); background: rgba(5,12,14,.58); box-shadow: 0 0 0 4px rgba(155,255,220,.055), inset 0 0 30px rgba(110,231,255,.025); outline: none; }
.enter { height: 68px; min-width: 132px; padding: 0 24px; border: 0; border-radius: 14px; color: #06110e; background: linear-gradient(110deg, #adffe2, #77eeff); font-weight: 720; cursor: pointer; box-shadow: 0 9px 30px rgba(110,231,255,.12); transition: transform .18s, box-shadow .18s, filter .18s; }
.enter:hover { transform: translateY(-2px); box-shadow: 0 13px 38px rgba(110,231,255,.21); filter: saturate(1.08); }
.enter:active { transform: translateY(0); }
.error { display: grid; grid-template-columns: auto 1fr; gap: 11px; align-items: start; margin: 14px 0 0; padding: 12px 14px; border: 1px solid rgba(255,154,154,.18); border-radius: 11px; color: #ffc2c2; background: rgba(255,84,84,.05); font-size: 13px; }
.error-mark { font: 12px/1.6 var(--mono); color: var(--danger); }
.hint { display: flex; align-items: center; gap: 8px; margin-top: 18px; color: var(--quiet); font-size: 12px; }
.hint svg { flex: 0 0 auto; }
.drop-form { margin-top: clamp(26px, 5vw, 38px); }
.field { margin-top: 18px; min-width: 0; }
.field-label { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; margin-bottom: 9px; font: 10px/1.4 var(--mono); color: var(--muted); text-transform: uppercase; letter-spacing: .15em; }
.field-label .optional { color: var(--quiet); letter-spacing: .08em; text-transform: none; }
.text-input, .select { width: 100%; height: 48px; padding: 0 14px; border: 1px solid var(--line); border-radius: 12px; color: var(--ink); caret-color: var(--mint); background: rgba(0,0,0,.23); font-size: 14px; transition: border-color .2s, box-shadow .2s, background .2s; }
.text-input:hover, .select:hover, .file-input:hover { border-color: rgba(155,255,220,.25); }
.text-input:focus, .select:focus, .file-input:focus { border-color: var(--line-hot); background: rgba(5,12,14,.58); box-shadow: 0 0 0 4px rgba(155,255,220,.055); outline: none; }
.text-input::placeholder { color: var(--quiet); }
.select { appearance: none; padding-right: 38px; cursor: pointer; background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none'%3E%3Cpath d='m6 9 6 6 6-6' stroke='%238fa2a2' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E"); background-repeat: no-repeat; background-position: right 14px center; }
.select option { color: var(--ink); background: var(--shell-solid); }
.option-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 14px; }
.file-input { display: block; width: 100%; padding: 20px 16px; border: 1px dashed rgba(155,255,220,.26); border-radius: 13px; color: var(--muted); background: rgba(155,255,220,.028); font-size: 13px; cursor: pointer; transition: border-color .2s, background .2s; }
.file-input::file-selector-button { margin-right: 14px; padding: 9px 15px; border: 1px solid rgba(155,255,220,.3); border-radius: 9px; color: var(--mint); background: rgba(155,255,220,.06); font: 600 12px/1 var(--sans); cursor: pointer; }
.field-note { margin: 9px 2px 0; color: var(--quiet); font-size: 11.5px; line-height: 1.5; }
.submit-drop { width: 100%; height: 58px; margin-top: 28px; }
.code-reveal { margin: clamp(26px, 5vw, 36px) 0 0; padding: 24px 16px; border: 1px solid rgba(155,255,220,.32); border-radius: 16px; color: var(--mint); background: linear-gradient(160deg, rgba(155,255,220,.07), rgba(110,231,255,.03)); font: 600 clamp(32px, 8vw, 52px)/1 var(--mono); letter-spacing: .32em; text-indent: .32em; text-align: center; text-shadow: 0 0 26px rgba(155,255,220,.3); user-select: all; }
.reveal-url { margin-top: 12px; padding: 13px 15px; border: 1px solid var(--line); border-radius: 12px; color: var(--muted); background: rgba(0,0,0,.2); font: 12px/1.5 var(--mono); text-align: center; overflow-wrap: anywhere; user-select: all; }
.reveal-block { margin-top: clamp(24px, 5vw, 34px); }
.reveal-block .field-label { margin: 0 0 9px; }
.reveal-block .code-reveal, .reveal-block .reveal-url { margin: 0; }
.reveal-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 1px; margin: 26px 0 0; border: 1px solid var(--line); border-radius: 13px; overflow: hidden; background: var(--line); }
.reveal-grid > div { min-width: 0; padding: 14px 15px; background: rgba(7,11,15,.92); }
.reveal-grid dt { color: var(--quiet); font: 9px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; }
.reveal-grid dd { margin: 8px 0 0; overflow: hidden; text-overflow: ellipsis; color: #bdcbc9; font: 11px/1.4 var(--mono); white-space: nowrap; }
.console-panel { margin-top: 34px; padding: 26px clamp(18px, 3vw, 28px) 30px; border: 1px solid var(--line); border-radius: 17px; background: rgba(8,12,17,.72); }
.panel-head h2 { margin: 0; font-size: 18px; letter-spacing: -.02em; font-weight: 600; }
.panel-head p { margin: 5px 0 0; color: var(--quiet); font-size: 12px; }
.drop-form.compact { margin-top: 6px; }
.empty-state { padding: 34px 20px; border: 1px dashed var(--line); border-radius: 15px; color: var(--quiet); text-align: center; font-size: 13px; }
.drop-shell .error { margin: 0 0 16px; }
.drop-row { grid-template-columns: minmax(0, 1fr) auto auto; }
.pills { display: flex; gap: 8px; flex-wrap: wrap; justify-content: flex-end; }
.pill { display: inline-flex; align-items: center; gap: 6px; padding: 6px 11px; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); background: rgba(255,255,255,.015); font: 10px/1 var(--mono); letter-spacing: .04em; white-space: nowrap; }
.pill-ok { color: var(--mint); border-color: rgba(155,255,220,.28); background: rgba(155,255,220,.05); }
.pill-warn { color: var(--amber); border-color: rgba(255,208,138,.32); background: rgba(255,208,138,.06); }
.pill-dead { color: var(--danger); border-color: rgba(255,154,154,.32); background: rgba(255,90,90,.06); }
.row-actions { display: flex; gap: 8px; align-items: center; }
.row-actions form { display: contents; }
button.entry-action, summary.entry-action { cursor: pointer; padding: 0 12px; background: rgba(255,255,255,.018); font-family: var(--sans); }
.entry-action.primary { color: #06110e; border: 0; background: linear-gradient(110deg, #adffe2, #77eeff); font-weight: 650; box-shadow: 0 6px 20px rgba(110,231,255,.14); }
.entry-action.primary:hover { color: #06110e; background: linear-gradient(110deg, #adffe2, #77eeff); filter: saturate(1.1); }
.entry-action.danger:hover { color: #ffc2c2; border-color: rgba(255,154,154,.4); background: rgba(255,90,90,.07); }
.confirm { position: relative; }
.confirm summary { list-style: none; user-select: none; }
.confirm summary::-webkit-details-marker { display: none; }
.confirm[open] summary { color: #ffc2c2; border-color: rgba(255,154,154,.4); }
.confirm-pop { position: absolute; right: 0; top: calc(100% + 8px); z-index: 6; width: 224px; padding: 13px; border: 1px solid rgba(255,154,154,.3); border-radius: 12px; background: var(--shell-solid); box-shadow: 0 20px 60px rgba(0,0,0,.55); }
.confirm-pop p { margin: 0 0 11px; color: var(--muted); font-size: 11px; line-height: 1.5; }
.danger-btn { width: 100%; padding: 10px; border: 1px solid rgba(255,154,154,.42); border-radius: 9px; color: #ffc2c2; background: rgba(255,90,90,.12); font-weight: 620; cursor: pointer; transition: background .18s; }
.danger-btn:hover { background: rgba(255,90,90,.2); }
button.bundle { cursor: pointer; font: inherit; font-weight: 650; }
.bundle.ghost { color: var(--muted); border-color: var(--line); background: rgba(255,255,255,.015); }
.bundle.ghost:hover { color: var(--mint); border-color: rgba(155,255,220,.4); background: rgba(155,255,220,.05); }
.reissue { margin-top: 34px; }
.reissue-body { padding: 4px 14px 18px; }
.reissue-note { margin: 4px 0 0; color: var(--quiet); font-size: 12px; max-width: 560px; }
.reissue .option-grid { margin-top: 2px; }
.reissue-submit { height: 50px; margin-top: 20px; }
.assure { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; margin-top: 12px; }
.assure > div { display: flex; gap: 10px; align-items: flex-start; min-width: 0; padding: 13px 15px; border: 1px solid var(--line); border-radius: 13px; background: rgba(8,13,18,.5); }
.assure > div > div { min-width: 0; }
.assure svg { flex: 0 0 auto; width: 15px; height: 15px; margin-top: 1px; color: var(--mint); opacity: .8; }
.assure b { display: block; color: #c4d2d0; font-size: 12px; font-weight: 560; }
.assure span { display: block; margin-top: 2px; color: var(--quiet); font-size: 11px; line-height: 1.45; }
.drop-shell { padding-top: 48px; }
.drop-hero { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 40px; align-items: end; padding: 36px 0; border-bottom: 1px solid var(--line); }
.drop-hero h1 { max-width: 760px; overflow-wrap: anywhere; }
.drop-sub { display: flex; flex-wrap: wrap; gap: 7px; margin: 20px 0 0; color: var(--muted); font-size: 13px; }
.drop-sub span { display: inline-flex; gap: 7px; align-items: center; }
.drop-sub b { color: #cedbd8; font-weight: 500; }
.bundle { display: inline-flex; align-items: center; justify-content: center; gap: 10px; min-height: 48px; padding: 0 18px; white-space: nowrap; border: 1px solid rgba(155,255,220,.29); border-radius: 12px; color: var(--mint); background: rgba(155,255,220,.055); text-decoration: none; font-weight: 650; transition: background .18s, border-color .18s, transform .18s; }
.bundle:hover { transform: translateY(-2px); border-color: rgba(155,255,220,.53); background: rgba(155,255,220,.09); }
.manifest-head { display: flex; align-items: end; justify-content: space-between; gap: 20px; margin: 38px 0 16px; }
.manifest-head h2 { margin: 0; font-size: 18px; letter-spacing: -.02em; font-weight: 600; }
.manifest-head p { margin: 4px 0 0; color: var(--quiet); font-size: 12px; }
.verified { display: inline-flex; align-items: center; gap: 8px; color: var(--mint); font-size: 11px; }
.file-list { overflow: hidden; border: 1px solid var(--line); border-radius: 17px; background: rgba(8,12,17,.72); box-shadow: 0 23px 70px rgba(0,0,0,.18); }
.entry { --indent: calc(min(var(--depth, 0), 6) * 18px); display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 18px; align-items: center; min-height: 62px; padding: 10px 16px 10px calc(16px + var(--indent)); border-bottom: 1px solid rgba(163,255,226,.075); }
.entry:last-child { border-bottom: 0; }
.entry:hover { background: linear-gradient(90deg, rgba(155,255,220,.035), transparent 74%); }
.entry-main { min-width: 0; display: flex; align-items: center; gap: 12px; }
.file-icon { width: 34px; height: 34px; flex: 0 0 auto; display: grid; place-items: center; color: var(--muted); border: 1px solid var(--line); border-radius: 9px; background: rgba(255,255,255,.018); }
.entry[data-cat="folder"] .file-icon { color: var(--violet); border-color: rgba(181,163,255,.17); background: rgba(181,163,255,.05); }
.entry[data-cat="image"] .file-icon,
.entry[data-cat="video"] .file-icon { color: var(--cyan); border-color: rgba(110,231,255,.16); background: rgba(110,231,255,.045); }
.entry[data-cat="audio"] .file-icon { color: var(--mint); border-color: rgba(155,255,220,.16); background: rgba(155,255,220,.045); }
.entry[data-cat="document"] .file-icon { color: var(--amber); border-color: rgba(255,208,138,.16); background: rgba(255,208,138,.04); }
.entry[data-cat="archive"] .file-icon { color: #e5b8ff; border-color: rgba(229,184,255,.16); background: rgba(229,184,255,.04); }
.entry[data-cat="code"] .file-icon { color: #a8e6ff; border-color: rgba(168,230,255,.16); background: rgba(168,230,255,.04); }
.twig { flex: 0 0 auto; width: 13px; height: 13px; margin: 0 -3px 0 -4px; color: rgba(163,255,226,.24); }
.file-name { min-width: 0; }
.file-name strong { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #dbe7e4; font-size: 13px; font-weight: 530; }
.file-name span { display: block; margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--quiet); font-size: 11px; }
.entry-size { color: var(--quiet); font: 10px/1 var(--mono); white-space: nowrap; }
.entry-action { min-width: 78px; min-height: 34px; display: inline-flex; align-items: center; justify-content: center; gap: 7px; border: 1px solid var(--line); border-radius: 9px; color: #b7c7c5; background: rgba(255,255,255,.018); text-decoration: none; font-size: 11px; transition: color .18s, border-color .18s, background .18s; }
.entry-action:hover { color: var(--mint); border-color: rgba(155,255,220,.36); background: rgba(155,255,220,.055); }
.entry-action.muted { visibility: hidden; }
.transfer-details { margin-top: 20px; border: 1px solid transparent; border-radius: 13px; color: var(--quiet); transition: border-color .18s, background .18s; }
.transfer-details[open] { border-color: var(--line); background: rgba(8,13,17,.6); }
.transfer-details summary { cursor: pointer; list-style: none; padding: 11px 13px; font-size: 12px; color: var(--muted); user-select: none; }
.transfer-details summary::-webkit-details-marker { display: none; }
.transfer-details summary::after { content: "+"; float: right; color: var(--mint); font: 14px/1 var(--mono); }
.transfer-details[open] summary::after { content: "−"; }
.detail-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 1px; border-top: 1px solid var(--line); background: var(--line); }
.detail-grid div { min-width: 0; padding: 13px; background: rgba(7,11,15,.92); }
.detail-grid dt { color: var(--quiet); font: 9px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; }
.detail-grid dd { margin: 7px 0 0; overflow: hidden; text-overflow: ellipsis; color: #bdcbc9; font: 10px/1.4 var(--mono); white-space: nowrap; }
.foot { display: flex; justify-content: space-between; align-items: baseline; gap: 28px; margin-top: 26px; color: var(--quiet); font-size: 11px; }
.foot p { margin: 0; max-width: 570px; }
.foot .right { text-align: right; white-space: nowrap; }
.foot a { color: var(--quiet); text-decoration: none; border-bottom: 1px solid rgba(163,255,226,.2); transition: color .18s, border-color .18s; }
.foot a:hover { color: var(--mint); border-color: rgba(155,255,220,.5); }
@media (max-width: 760px) {
  .page { width: min(100% - 28px, 1120px); padding-top: 18px; }
  .landing { padding: 26px 0 34px; }
  .artifact-panel { padding: 30px 22px; }
  .code-field { grid-template-columns: 1fr; }
  .assure { grid-template-columns: 1fr; gap: 8px; }
  .option-grid { grid-template-columns: 1fr; }
  .code-reveal { letter-spacing: .2em; text-indent: .2em; }
  .reveal-grid { grid-template-columns: 1fr; }
  .enter { width: 100%; height: 56px; }
  .code-input { height: 64px; padding-left: 20px; letter-spacing: .39em; }
  .microgrid { grid-template-columns: 1fr; }
  .micro { min-height: 55px; }
  .drop-shell { padding-top: 34px; }
  .drop-hero { grid-template-columns: 1fr; align-items: start; gap: 24px; }
  .bundle { width: 100%; }
  .entry { grid-template-columns: minmax(0, 1fr) auto; gap: 10px; padding-right: 10px; }
  .entry-size { display: none; }
  .entry-action { min-width: 40px; width: 40px; padding: 0; }
  .entry-action .action-label { display: none; }
  .manifest-head { align-items: start; }
  .verified { display: none; }
  .foot { flex-direction: column; }
  .foot .right { text-align: left; }
  .detail-grid { grid-template-columns: 1fr 1fr; }
  .entry.drop-row { grid-template-columns: 1fr; gap: 12px; padding: 14px 12px 16px; }
  .drop-row .pills { justify-content: flex-start; }
  .drop-row .row-actions { justify-content: flex-start; }
  .drop-row .entry-action { font-size: 11px; min-width: 0; width: auto; padding: 0 14px; min-height: 36px; white-space: nowrap; }
  .drop-row .confirm-pop { right: auto; left: 0; }
  .drop-hero form { width: 100%; }
}
@media (max-width: 390px) {
  .mast .link-state { display: none; }
  .entry { --indent: calc(min(var(--depth), 3) * 10px); padding-left: calc(10px + var(--indent)); }
  .file-icon { width: 30px; height: 30px; }
}
@media (prefers-reduced-motion: reduce) {
  html { scroll-behavior: auto; }
  *, *::before, *::after { scroll-behavior: auto !important; transition-duration: .001ms !important; animation-duration: .001ms !important; animation-iteration-count: 1 !important; }
}
@media (prefers-contrast: more) {
  :root { --line: rgba(200,255,237,.33); --muted: #b6c4c2; --quiet: #92a3a1; }
  .artifact-panel, .file-list { background: #080d11; }
}
@media print {
  html, body { background: #fff; color: #000; }
  body::before, body::after, .ambient, .skip, .link-state, .foot, .assure, .bundle, .enter { display: none !important; }
  .page { width: 100%; padding: 0; min-height: 0; }
  .artifact { width: 100%; }
  .frame::before, .artifact-panel::after { display: none; }
  .artifact-panel { padding: 0; border-radius: 0; background: none; box-shadow: none; backdrop-filter: none; }
  .eyebrow, .wordmark span { color: #000; }
  .lede, .reveal-grid dt, .reveal-grid dd, .field-label { color: #333; }
  .code-reveal { border: 1px solid #000; color: #000; background: none; text-shadow: none; }
  .reveal-url, .reveal-grid, .reveal-grid > div { border-color: #999; color: #000; background: none; }
}
"#;

pub fn landing(base_path: &str, error: bool, prefill: Option<&str>) -> Markup {
    page(
        "FrankenFile · Receive",
        html! {
            a class="skip" href="#signal" { "Skip to code entry" }
            (ambient())
            main class="page" id="main" {
                (mast(base_path))
                section class="landing" aria-labelledby="page-title" {
                    div class="artifact" {
                        div class="frame" {
                        div class="artifact-panel" {
                            p class="eyebrow" { "Private file drop" }
                            h1 id="page-title" { "Open your " span class="spectral" { "files" } }
                            @if prefill.is_some() && !error {
                                p class="lede" { "This link carries a pickup code — it’s already filled in. Press Open files to unlock the drop on this device." }
                            } @else {
                                p class="lede" { "Enter the 6-character code you were given. No account, app, or sign-in needed." }
                            }
                            form class="redeem" method="post" action=(format!("{base_path}/redeem")) novalidate {
                                div class="code-label" {
                                    label for="signal" { "6-character code" }
                                    span { "Letters and numbers" }
                                }
                                div class="code-field" {
                                    input class="code-input" id="signal" name="code" type="text"
                                        value=[prefill]
                                        autocomplete="one-time-code" pattern="[0-9A-Za-z]{6}"
                                        minlength="6" maxlength="6" spellcheck="false" autocapitalize="characters"
                                        autocorrect="off" enterkeyhint="go"
                                        aria-describedby="code-hint" aria-invalid=(if error { "true" } else { "false" })
                                        autofocus[prefill.is_none()];
                                    button class="enter" type="submit" autofocus[prefill.is_some()] { "Open files" }
                                }
                                @if error {
                                    div class="error" role="alert" {
                                        span class="error-mark" aria-hidden="true" { "◇" }
                                        span { "That code didn’t work. Check it for typos, or ask the sender for a fresh one — codes expire quickly." }
                                    }
                                }
                                p class="hint" id="code-hint" {
                                    (shield_icon())
                                    "Case doesn’t matter. Your code unlocks this drop on this device only, then expires."
                                }
                            }
                        }
                        }
                        (assurances())
                    }
                }
                footer class="foot" {
                    p { "Codes and drops expire automatically. Nothing is kept after that." }
                    p class="right" { a href=(format!("{base_path}/drop")) { "Operator console" } }
                }
            }
        },
    )
}

pub fn frankendrop_unlock(base_path: &str, error: Option<&str>) -> Markup {
    page(
        "FrankenDrop · Operator console",
        html! {
            a class="skip" href="#password" { "Skip to unlock form" }
            (ambient())
            main class="page" id="main" {
                (mast(base_path))
                section class="landing" aria-labelledby="page-title" {
                    div class="artifact frame" {
                        div class="artifact-panel" {
                            p class="eyebrow" { "Operator console" }
                            h1 id="page-title" { "Franken" span class="spectral" { "Drop" } }
                            p class="lede" { "Unlock the console to publish drops, watch code and expiry status, reissue pickup codes, and revoke access early." }
                            form class="drop-form" method="post" action=(format!("{base_path}/drop/unlock")) {
                                div class="field" {
                                    label class="field-label" for="password" { span { "Operator password" } }
                                    input class="text-input" id="password" type="password" name="password"
                                        autocomplete="current-password" required autofocus
                                        aria-invalid=(if error.is_some() { "true" } else { "false" });
                                }
                                @if let Some(message) = error {
                                    div class="error" role="alert" {
                                        span class="error-mark" aria-hidden="true" { "◇" }
                                        span { (message) }
                                    }
                                }
                                button class="enter submit-drop" type="submit" { "Unlock console" }
                                p class="hint" {
                                    (shield_icon())
                                    "The console stays unlocked on this device for 30 minutes, then locks itself."
                                }
                            }
                        }
                    }
                }
                footer class="foot" {
                    p { "Wrong attempts share the same rate-limit budget as pickup-code guessing." }
                    p class="right" { a href=(base_path) { "Receiver page" } }
                }
            }
        },
    )
}

pub struct ConsoleView<'a> {
    pub drops: &'a [DropRecord],
    pub now: i64,
    pub error: Option<&'a str>,
    pub reissue_error: Option<&'a str>,
}

/// True when a drop wants operator input soon: its pickup code is already dead,
/// or the drop itself is inside its final six hours.
fn needs_attention(drop: &DropRecord, now: i64) -> bool {
    drop.code_expires_at <= now || drop.expires_at - now <= 6 * 3600
}

pub fn frankendrop_console(base_path: &str, view: &ConsoleView) -> Markup {
    let active = view.drops.len();
    let attention = view
        .drops
        .iter()
        .filter(|drop| needs_attention(drop, view.now))
        .count();
    // Surface the drops an operator may still have to act on before the calm
    // ones, then order each group by how soon it disappears.
    let mut ordered: Vec<&DropRecord> = view.drops.iter().collect();
    ordered.sort_by_key(|drop| {
        (
            !needs_attention(drop, view.now),
            drop.code_expires_at.min(drop.expires_at),
        )
    });
    page(
        "FrankenDrop · Console",
        html! {
            a class="skip" href="#drops" { "Skip to active drops" }
            (ambient())
            main class="page" id="main" {
                (mast(base_path))
                section class="drop-shell" aria-labelledby="page-title" {
                    div class="drop-hero" {
                        div {
                            p class="eyebrow" { "Operator console · Unlocked" }
                            h1 id="page-title" { "Franken" span class="spectral" { "Drop" } }
                            div class="drop-sub" {
                                span { b { (active) } (if active == 1 { " active drop" } else { " active drops" }) }
                                @if attention > 0 {
                                    span aria-hidden="true" { "·" }
                                    span { b { (attention) } (if attention == 1 { " needs attention" } else { " need attention" }) }
                                }
                                span aria-hidden="true" { "·" }
                                span { "Locks itself after 30 minutes" }
                            }
                        }
                        form method="post" action=(format!("{base_path}/drop/lock")) {
                            button class="bundle ghost" type="submit" { (lock_icon()) "Lock console" }
                        }
                    }
                    div class="manifest-head" id="drops" {
                        div {
                            h2 { "Active drops" }
                            p { "Everything actionable at a glance — reissue a dead code or revoke a drop early." }
                        }
                        span class="verified" { (verified_icon()) "Live status" }
                    }
                    @if let Some(message) = view.error {
                        div class="error" role="alert" {
                            span class="error-mark" aria-hidden="true" { "◇" }
                            span { (message) }
                        }
                    }
                    @if ordered.is_empty() {
                        div class="empty-state" {
                            "Nothing is live right now. Publish a drop below and its pickup code appears once, on the next screen."
                        }
                    } @else {
                        div class="file-list" role="list" {
                            @for drop in &ordered {
                                (console_row(base_path, drop, view.now))
                            }
                        }
                    }
                    section class="console-panel" id="publish" aria-labelledby="publish-title" {
                        div class="panel-head" {
                            h2 id="publish-title" { "Publish a new drop" }
                            p { "Uploads are snapshotted into immutable, expiring storage; the pickup code is shown once." }
                        }
                            form class="drop-form compact" method="post" action=(format!("{base_path}/drop")) enctype="multipart/form-data" {
                                div class="field" {
                                    label class="field-label" for="files" { span { "Files to host" } span class="optional" { "Up to 400 files" } }
                                    input class="file-input" id="files" type="file" name="files" multiple required
                                        aria-describedby="files-hint";
                                    p class="field-note" id="files-hint" { "Choose files or drag them onto this box. Everything is captured as one immutable snapshot." }
                                }
                                div class="field" {
                                    label class="field-label" for="title" { span { "Title" } span class="optional" { "Optional" } }
                                    input class="text-input" id="title" type="text" name="title" maxlength="120"
                                        placeholder="What the recipient will see" spellcheck="false";
                                }
                                div class="option-grid" {
                                    div class="field" {
                                        label class="field-label" for="drop_ttl" { span { "Available for" } }
                                        select class="select" id="drop_ttl" name="drop_ttl" {
                                            option value="1h" { "1 hour" }
                                            option value="6h" { "6 hours" }
                                            option value="24h" selected { "24 hours" }
                                            option value="3d" { "3 days" }
                                            option value="7d" { "7 days" }
                                            option value="30d" { "30 days" }
                                        }
                                    }
                                    div class="field" {
                                        label class="field-label" for="code_ttl" { span { "Code redeemable for" } }
                                        select class="select" id="code_ttl" name="code_ttl" {
                                            option value="15m" selected { "15 minutes" }
                                            option value="1h" { "1 hour" }
                                            option value="6h" { "6 hours" }
                                            option value="24h" { "24 hours" }
                                        }
                                    }
                                    div class="field" {
                                        label class="field-label" for="max_redemptions" { span { "Redemption limit" } }
                                        select class="select" id="max_redemptions" name="max_redemptions" {
                                            option value="" selected { "Unlimited" }
                                            option value="1" { "1 device" }
                                            option value="3" { "3 devices" }
                                            option value="10" { "10 devices" }
                                            option value="25" { "25 devices" }
                                        }
                                    }
                                    div class="field" {
                                        label class="field-label" for="code_style" { span { "Code style" } }
                                        select class="select" id="code_style" name="code_style" {
                                            option value="alnum" selected { "Letters + numbers" }
                                            option value="digits" { "Numbers only" }
                                        }
                                    }
                                }
                                button class="enter submit-drop" type="submit" { "Publish drop" }
                                p class="hint" {
                                    (shield_icon())
                                    "The pickup code is shown once after publishing. Recipients never see server paths."
                                }
                            }
                    }
                    details class="transfer-details reissue" open[view.reissue_error.is_some()] {
                        summary { "Reissue by drop reference" }
                        div class="reissue-body" {
                            p class="reissue-note" { "Handy when you only have a reference from a receipt or the CLI. Reissuing mints a fresh code and immediately retires the old one; devices that already redeemed keep their access." }
                                    form method="post" action=(format!("{base_path}/drop/recode")) {
                                        div class="option-grid" {
                                            div class="field" {
                                                label class="field-label" for="reference" { span { "Drop reference" } span class="optional" { "ID or 8+ char prefix" } }
                                                input class="text-input" id="reference" type="text" name="reference"
                                                    minlength="8" maxlength="32" spellcheck="false" autocapitalize="off"
                                                    placeholder="From the receipt or CLI" required;
                                            }
                                            div class="field" {
                                                label class="field-label" for="reissue-code-ttl" { span { "New code redeemable for" } }
                                                select class="select" id="reissue-code-ttl" name="code_ttl" {
                                                    option value="15m" selected { "15 minutes" }
                                                    option value="1h" { "1 hour" }
                                                    option value="6h" { "6 hours" }
                                                    option value="24h" { "24 hours" }
                                                }
                                            }
                                            div class="field" {
                                                label class="field-label" for="reissue-code-style" { span { "Code style" } }
                                                select class="select" id="reissue-code-style" name="code_style" {
                                                    option value="alnum" selected { "Letters + numbers" }
                                                    option value="digits" { "Numbers only" }
                                                }
                                            }
                                        }
                                        @if let Some(message) = view.reissue_error {
                                            div class="error" role="alert" {
                                                span class="error-mark" aria-hidden="true" { "◇" }
                                                span { (message) }
                                            }
                                        }
                                        button class="enter submit-drop reissue-submit" type="submit" { "Reissue code" }
                                    }
                        }
                    }
                    footer class="foot" {
                        p { "Uploads are content-addressed, integrity-checked, and garbage-collected after expiry. Manage from the CLI with frankenfile list · recode · revoke." }
                        p class="right" { a href=(base_path) { "Receiver page" } }
                    }
                }
            }
        },
    )
}

fn console_row(base_path: &str, drop: &DropRecord, now: i64) -> Markup {
    let code_live = drop.code_expires_at > now;
    let drop_left = drop.expires_at.saturating_sub(now);
    let drop_ending = drop_left <= 6 * 3600;
    let redemption_note = match drop.max_redemptions {
        Some(max) => format!("{} of {max} redemptions", drop.redemption_count),
        None => format!("{} redemptions", drop.redemption_count),
    };
    html! {
        div class="entry drop-row" role="listitem" {
            div class="entry-main" {
                span class="file-icon" aria-hidden="true" { (archive_icon()) }
                div class="file-name" {
                    strong title=(drop.id) { (drop.title) }
                    span {
                        (human_size(drop.total_bytes)) " · "
                        (drop.file_count) (if drop.file_count == 1 { " file" } else { " files" }) " · "
                        (redemption_note) " · " (short_id(&drop.id))
                    }
                }
            }
            div class="pills" {
                @if code_live {
                    span class="pill pill-ok" { "Code live · " (human_remaining(drop.code_expires_at - now)) " left" }
                } @else {
                    span class="pill pill-dead" { "Code expired" }
                }
                @if drop_ending {
                    span class="pill pill-warn" { "Drop ends in " (human_remaining(drop_left)) }
                } @else {
                    span class="pill" { "Drop · " (human_remaining(drop_left)) " left" }
                }
            }
            div class="row-actions" {
                form method="post" action=(format!("{base_path}/drop/recode")) {
                    input type="hidden" name="reference" value=(drop.id);
                    button class=(if code_live { "entry-action" } else { "entry-action primary" }) type="submit"
                        aria-label=(format!("Issue a new pickup code for {}", drop.title)) {
                        (refresh_icon()) "New code"
                    }
                }
                details class="confirm" {
                    summary class="entry-action danger" aria-label=(format!("Revoke {}", drop.title)) { "Revoke" }
                    div class="confirm-pop" {
                        p { "Immediately invalidates the code and every session for this drop." }
                        form method="post" action=(format!("{base_path}/drop/revoke")) {
                            input type="hidden" name="reference" value=(drop.id);
                            button class="danger-btn" type="submit" { "Revoke now" }
                        }
                    }
                }
            }
        }
    }
}

fn human_remaining(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 3600 {
        format!("{}m", (seconds / 60).max(1))
    } else if seconds < 86400 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    }
}

pub fn frankendrop_created(base_path: &str, result: &CreateResult) -> Markup {
    drop_receipt(
        base_path,
        result,
        "Drop published",
        "Share this pickup code now — it is shown only once and never stored in plain text.",
    )
}

pub fn frankendrop_recoded(base_path: &str, result: &CreateResult) -> Markup {
    drop_receipt(
        base_path,
        result,
        "Code reissued",
        "The previous code no longer works. Share this new code — it is shown only once and never stored in plain text.",
    )
}

fn drop_receipt(base_path: &str, result: &CreateResult, eyebrow: &str, lede: &str) -> Markup {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let payload = format!(
        "{} · {} file{}",
        human_size(result.total_bytes),
        result.file_count,
        if result.file_count == 1 { "" } else { "s" }
    );
    page(
        &format!("FrankenDrop · {eyebrow}"),
        html! {
            (ambient())
            main class="page" {
                (mast(base_path))
                section class="landing" aria-labelledby="page-title" {
                    div class="artifact frame" {
                        div class="artifact-panel" {
                            p class="eyebrow" { (eyebrow) }
                            h1 id="page-title" { (result.title) }
                            p class="lede" { (lede) }
                            div class="reveal-block" {
                                p class="field-label" {
                                    span { "Pickup code" }
                                    span class="optional" { "Tap to select" }
                                }
                                p class="code-reveal" aria-label=(format!("Pickup code {}", result.code)) { (result.code) }
                            }
                            div class="reveal-block" {
                                p class="field-label" {
                                    span { "Share link" }
                                    span class="optional" { "Opens with the code filled in" }
                                }
                                p class="reveal-url" { (result.url) "/" (result.code) }
                            }
                            dl class="reveal-grid" {
                                div { dt { "Payload" } dd { (payload) } }
                                div { dt { "Code redeemable for" } dd { (human_remaining(result.code_expires_at - now)) " · until " (format_timestamp(result.code_expires_at)) } }
                                div { dt { "Drop available for" } dd { (human_remaining(result.drop_expires_at - now)) " · until " (format_timestamp(result.drop_expires_at)) } }
                                div { dt { "Reference" } dd title=(result.drop_id) { (short_id(&result.drop_id)) } }
                            }
                            a class="bundle" href=(format!("{base_path}/drop")) style="margin-top:28px" { "Create another drop" }
                        }
                    }
                }
                footer class="foot" {
                    p { "Manage drops from the CLI: frankenfile list · show · revoke." }
                    p class="right" { a href=(base_path) { "Receiver page" } }
                }
            }
        },
    )
}

pub fn drop_page(base_path: &str, detail: &DropDetail) -> Markup {
    let d = &detail.drop;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let remaining = d.expires_at.saturating_sub(now);
    let ending_soon = remaining <= 6 * 3600;
    let expires = format_timestamp(d.expires_at);
    let created = format_timestamp(d.created_at);
    let fingerprint = d.manifest_hash.get(..16).unwrap_or(&d.manifest_hash);
    page(
        &format!("{} · FrankenFile", d.title),
        html! {
            a class="skip" href="#manifest" { "Skip to files" }
            (ambient())
            main class="page" {
                (mast(base_path))
                section class="drop-shell" aria-labelledby="drop-title" {
                    div class="drop-hero" {
                        div {
                            p class="eyebrow" { "Ready to download" }
                            h1 id="drop-title" { (d.title) }
                            div class="drop-sub" {
                                span { b { (d.file_count) } (if d.file_count == 1 { " file" } else { " files" }) }
                                span aria-hidden="true" { "·" }
                                span { b { (human_size(d.total_bytes)) } }
                                span aria-hidden="true" { "·" }
                                @if ending_soon {
                                    span class="pill pill-warn" { (clock_icon()) (human_remaining(remaining)) " left" }
                                } @else {
                                    span { "Available for " b { (human_remaining(remaining)) } }
                                }
                            }
                        }
                        a class="bundle" href=(format!("{base_path}/d/{}/bundle", d.id)) {
                            (download_icon()) "Download all"
                        }
                    }
                    div class="manifest-head" id="manifest" {
                        div {
                            h2 { "Files" }
                            p { "Take one file, one folder as a ZIP, or the whole drop at once." }
                        }
                        span class="verified" { (verified_icon()) "Integrity checked" }
                    }
                    div class="file-list" role="list" {
                        @for entry in &detail.entries {
                            (entry_row(base_path, &d.id, entry))
                        }
                    }
                    details class="transfer-details" {
                        summary { "Transfer details" }
                        dl class="detail-grid" {
                            div { dt { "Created" } dd { (created) } }
                            div { dt { "Available until" } dd { (expires) } }
                            div { dt { "Reference" } dd { (short_id(&d.id)) } }
                            div { dt { "Integrity" } dd title=(d.manifest_hash) { (fingerprint) } }
                        }
                    }
                    footer class="foot" {
                        p { "Downloads resume safely — if one is interrupted, your browser can continue it while the drop is still available." }
                        p class="right" { "FrankenFile" }
                    }
                }
            }
        },
    )
}

pub fn service_error(base_path: &str) -> Markup {
    page(
        "FrankenFile · Signal interrupted",
        html! {
            (ambient())
            main class="page" {
                (mast(base_path))
                section class="landing" {
                    div class="artifact frame" { div class="artifact-panel" {
                    p class="eyebrow" { "Drop unavailable" }
                    h1 { "These files can’t be " span class="spectral" { "opened" } }
                    p class="lede" { "The drop may have expired or the link may be incomplete. Ask the sender for a new code if you still need the files." }
                    a class="bundle" href=(base_path) style="margin-top:32px" { "Try another code" }
                    } }
                }
            }
        },
    )
}

/// Wraps a page body in the shared document shell.
///
/// Every page is deliberately self-contained: no script, no external asset, and
/// no per-drop metadata in the head, so a shared link previews identically
/// whatever it points at.
fn page(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover";
                meta name="theme-color" content="#05070a";
                meta name="color-scheme" content="dark";
                meta name="robots" content="noindex,nofollow,noarchive";
                meta name="description" content="A private, expiring file drop. Enter the code you were given to open the files.";
                meta property="og:type" content="website";
                meta property="og:title" content="FrankenFile";
                meta property="og:description" content="A private, expiring file drop. Enter the code you were given to open the files.";
                title { (title) }
                link rel="icon" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'%3E%3Cpath d='M16 2.5 28 9.4v13.2L16 29.5 4 22.6V9.4Z' fill='%23070d11' stroke='%239bffdc'/%3E%3Cpath d='m11 13 5-3 5 3v6l-5 3-5-3Z' fill='%236ee7ff' fill-opacity='.35' stroke='%239bffdc'/%3E%3C/svg%3E";
                style { (PreEscaped(CSS)) }
            }
            body { (body) }
        }
    }
}

/// Quiet reassurance strip under the receiver panel: what a first-time
/// recipient needs to know before typing a code they were handed.
fn assurances() -> Markup {
    html! {
        div class="assure" {
            div {
                (clock_icon())
                div {
                    b { "Expires on its own" }
                    span { "The code and the files both stop working after a time the sender chose." }
                }
            }
            div {
                (shield_icon())
                div {
                    b { "No account needed" }
                    span { "Nothing to install or sign up for, and no tracking of any kind." }
                }
            }
            div {
                (verified_icon())
                div {
                    b { "Integrity checked" }
                    span { "Every file is fingerprinted, so what you download is exactly what was sent." }
                }
            }
        }
    }
}

fn ambient() -> Markup {
    html! { div class="ambient" aria-hidden="true" { i class="orb orb-a" {} i class="orb orb-b" {} i class="beam" {} } }
}

fn mast(base_path: &str) -> Markup {
    html! {
        header class="mast" {
            a class="identity" href=(base_path) aria-label="FrankenFile receiver" {
                svg class="sigil" viewBox="0 0 32 32" fill="none" aria-hidden="true" {
                    path d="M16 2.5 28.2 9.4v13.2L16 29.5 3.8 22.6V9.4L16 2.5Z" stroke="url(#s)" stroke-width="1.2" {}
                    path d="m9.4 13.1 6.6-3.8 6.6 3.8v7.7L16 24.6l-6.6-3.8v-7.7Z" stroke="#9bffdc" stroke-opacity=".55" {}
                    path d="m12.7 15 3.3-1.9 3.3 1.9v3.9L16 20.8l-3.3-1.9V15Z" fill="#9bffdc" fill-opacity=".24" stroke="#9bffdc" {}
                    defs { linearGradient id="s" x1="4" y1="4" x2="28" y2="28" { stop stop-color="#9bffdc" {} stop offset=".5" stop-color="#6ee7ff" {} stop offset="1" stop-color="#b5a3ff" {} } }
                }
                span class="wordmark" { "Franken" span { "File" } }
            }
            span class="link-state" { (shield_icon()) "Private transfer" }
        }
    }
}

fn entry_row(base_path: &str, drop_id: &str, entry: &Entry) -> Markup {
    let depth = entry.path.matches('/').count();
    let kind = if entry.kind == EntryKind::File {
        "file"
    } else {
        "directory"
    };
    let category = categorize(entry);
    let description = entry_description(entry, category);
    html! {
        div class="entry" role="listitem" data-kind=(kind) data-cat=(category.slug()) style=(format!("--depth:{depth}")) {
            div class="entry-main" {
                @if depth > 0 { (twig_icon()) }
                span class="file-icon" aria-hidden="true" { (category_icon(category)) }
                div class="file-name" {
                    strong title=(entry.path) { (entry.filename()) }
                    span { (description) }
                }
            }
            span class="entry-size" { @if entry.kind == EntryKind::File { (human_size(entry.size)) } @else { "—" } }
            @if entry.kind == EntryKind::File {
                a class="entry-action" href=(format!("{base_path}/d/{drop_id}/file/{}", entry.id)) aria-label=(format!("Download {}", entry.filename())) {
                    (download_icon()) span class="action-label" { "Download" }
                }
            } @else if entry.is_top_level() {
                a class="entry-action" href=(format!("{base_path}/d/{drop_id}/folder/{}", entry.id)) aria-label=(format!("Download folder {} as ZIP", entry.filename())) {
                    (archive_icon()) span class="action-label" { "ZIP" }
                }
            } @else {
                span class="entry-action muted" aria-hidden="true" {}
            }
        }
    }
}

/// Coarse presentation category for a manifest entry. Drives both the row icon
/// and its one-word description, so the two can never disagree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Code,
    Text,
    Generic,
}

impl Category {
    fn slug(self) -> &'static str {
        match self {
            Category::Folder => "folder",
            Category::Image => "image",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Document => "document",
            Category::Archive => "archive",
            Category::Code => "code",
            Category::Text => "text",
            Category::Generic => "file",
        }
    }
}

/// Classify from the recorded media type first, then from the file extension —
/// the extension is only ever read for display and never for access decisions.
fn categorize(entry: &Entry) -> Category {
    if entry.kind == EntryKind::Directory {
        return Category::Folder;
    }
    match entry.media_type.as_deref().unwrap_or_default() {
        "application/pdf" | "application/msword" | "application/rtf" => return Category::Document,
        "application/zip" | "application/gzip" | "application/x-tar" => return Category::Archive,
        "application/json" | "application/xml" | "text/html" | "text/css" => {
            return Category::Code;
        }
        "text/markdown" | "text/plain" | "text/csv" => return Category::Text,
        mime if mime.starts_with("image/") => return Category::Image,
        mime if mime.starts_with("video/") => return Category::Video,
        mime if mime.starts_with("audio/") => return Category::Audio,
        mime if mime.starts_with("application/vnd.openxmlformats") => return Category::Document,
        _ => {}
    }
    let extension = entry
        .path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "heic" | "bmp" | "tif"
        | "tiff" => Category::Image,
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => Category::Video,
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "opus" => Category::Audio,
        "pdf" | "doc" | "docx" | "odt" | "ppt" | "pptx" | "xls" | "xlsx" | "ods" | "epub" => {
            Category::Document
        }
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => Category::Archive,
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "h" | "cpp" | "java" | "rb"
        | "sh" | "json" | "toml" | "yaml" | "yml" | "html" | "css" | "sql" => Category::Code,
        "txt" | "md" | "csv" | "log" | "rtf" => Category::Text,
        _ => Category::Generic,
    }
}

fn category_icon(category: Category) -> Markup {
    match category {
        Category::Folder => html! {
            svg width="17" height="17" viewBox="0 0 24 24" fill="none" { path d="M3.5 6.5h6l2 2h9v10h-17z" stroke="currentColor" stroke-width="1.5" {} }
        },
        Category::Image => html! {
            svg width="17" height="17" viewBox="0 0 24 24" fill="none" { rect x="3.4" y="5.4" width="17.2" height="13.2" rx="2.2" stroke="currentColor" stroke-width="1.5" {} circle cx="8.9" cy="10.1" r="1.5" fill="currentColor" {} path d="m4.6 17.4 4.8-4.6 3.2 2.8 3-2.6 4.4 4.4" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round" {} }
        },
        Category::Video => html! {
            svg width="17" height="17" viewBox="0 0 24 24" fill="none" { rect x="3.4" y="5.4" width="17.2" height="13.2" rx="2.2" stroke="currentColor" stroke-width="1.5" {} path d="M10.3 8.9 15.9 12l-5.6 3.1z" fill="currentColor" {} }
        },
        Category::Audio => html! {
            svg width="17" height="17" viewBox="0 0 24 24" fill="none" { path d="M5.5 10.2v3.6M9.8 6.6v10.8M14.2 8.8v6.4M18.5 10.9v2.2" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" {} }
        },
        Category::Document => html! {
            svg width="16" height="16" viewBox="0 0 24 24" fill="none" { path d="M7 3.5h6.8L18 7.7v12.8H7z" stroke="currentColor" stroke-width="1.5" {} path d="M13.5 3.8V8h4.2M9.6 12.5h6M9.6 16h4.2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" {} }
        },
        Category::Archive => html! {
            svg width="15" height="15" viewBox="0 0 24 24" fill="none" { path d="M5 8h14v12H5zM4 4h16v4H4zM10 12h4" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" {} }
        },
        Category::Code => html! {
            svg width="16" height="16" viewBox="0 0 24 24" fill="none" { path d="m9 8-4.5 4L9 16m6-8 4.5 4L15 16m-2.2-9.5-1.6 11" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" {} }
        },
        Category::Text => html! {
            svg width="16" height="16" viewBox="0 0 24 24" fill="none" { path d="M7 3.5h6.8L18 7.7v12.8H7z" stroke="currentColor" stroke-width="1.5" {} path d="M13.5 3.8V8h4.2M9.6 11.5h5M9.6 14.6h5M9.6 17.7h3" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" {} }
        },
        Category::Generic => html! {
            svg width="16" height="16" viewBox="0 0 24 24" fill="none" { path d="M7 3.5h6.8L18 7.7v12.8H7z" stroke="currentColor" stroke-width="1.5" {} path d="M13.5 3.8V8h4.2" stroke="currentColor" stroke-width="1.5" {} }
        },
    }
}

/// Elbow connector that makes folder nesting readable at a glance.
fn twig_icon() -> Markup {
    html! { svg class="twig" viewBox="0 0 16 16" fill="none" aria-hidden="true" { path d="M4 1v9.5a2 2 0 0 0 2 2h6" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" {} } }
}

fn download_icon() -> Markup {
    html! { svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="M12 3v12m0 0 4-4m-4 4-4-4M5 20h14" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" {} } }
}

fn archive_icon() -> Markup {
    html! { svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="M5 8h14v12H5zM4 4h16v4H4zM10 12h4" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" {} } }
}

fn refresh_icon() -> Markup {
    html! { svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="M20 12a8 8 0 1 1-2.34-5.66M20 3v5h-5" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" {} } }
}

fn lock_icon() -> Markup {
    html! { svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="M6 11h12v9H6zM8.5 11V7.5a3.5 3.5 0 0 1 7 0V11" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" {} } }
}

fn clock_icon() -> Markup {
    html! { svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true" { circle cx="12" cy="12" r="8.5" stroke="currentColor" stroke-width="1.5" {} path d="M12 7.4V12l3.2 2.2" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" {} } }
}

fn shield_icon() -> Markup {
    html! { svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="M12 3 19 6v5.2c0 4.4-2.9 7.6-7 9.8-4.1-2.2-7-5.4-7-9.8V6z" stroke="currentColor" stroke-width="1.5" {} path d="m9 12 2 2 4-4" stroke="currentColor" stroke-width="1.5" {} } }
}

fn verified_icon() -> Markup {
    html! { svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true" { path d="m12 2.8 2.2 2 3-.3.8 3 2.5 1.6-1.2 2.8 1.2 2.8-2.5 1.6-.8 3-3-.3-2.2 2-2.2-2-3 .3-.8-3-2.5-1.6 1.2-2.8-1.2-2.8L6 7.5l.8-3 3 .3z" stroke="currentColor" stroke-width="1.3" {} path d="m9 12 2 2 4-4" stroke="currentColor" stroke-width="1.5" {} } }
}

fn format_timestamp(timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|time| {
            time.format(format_description!(
                "[month repr:short] [day padding:none], [hour]:[minute] UTC"
            ))
            .ok()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Secondary row line: what the entry is, plus the folder it came from.
fn entry_description(entry: &Entry, category: Category) -> String {
    if category == Category::Folder {
        return "Folder".to_string();
    }
    let kind = match entry.media_type.as_deref().unwrap_or_default() {
        "text/markdown" => "Markdown",
        "text/plain" => "Text",
        "application/pdf" => "PDF",
        "application/zip" => "ZIP archive",
        "application/json" => "JSON",
        _ => match category {
            Category::Image => "Image",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Document => "Document",
            Category::Archive => "Archive",
            Category::Code => "Code",
            Category::Text => "Text",
            _ => "File",
        },
    };
    match entry.path.rsplit_once('/') {
        Some((parent, _)) => format!("{kind} · {parent}"),
        None => kind.to_string(),
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn short_id(id: &str) -> &str {
    id.get(..10).unwrap_or(id)
}
