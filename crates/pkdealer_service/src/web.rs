#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! HTTP spectator interface for the `PKDealer` service.
//!
//! Provides two routes:
//! - `GET /` — embedded HTML page driven by the browser `EventSource` API
//! - `GET /events` — Server-Sent Events stream; each event is a JSON [`SpectatorEvent`]
//!
//! The spectator page renders an SVG poker table (derived from the `pkarena0-web`
//! project) and updates it live via SSE. Hole cards are currently shown face-down
//! because the broadcast channel carries `CardVisibility::Hidden` snapshots; full
//! card visibility can be added by wiring a spectator-token channel later.

use std::{convert::Infallible, sync::Arc};

use axum::{
    Router,
    extract::State,
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use pkdealer_proto::dealer::{
    EventType, PlayerState as ProtoPlayerState, Street, TableEvent, TableStatus,
};
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;

// ── HTML page ─────────────────────────────────────────────────────────────────

const SPECTATOR_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>PKDealer Spectator</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: #0d0d1a;
      color: #ccc;
      font-family: 'Trebuchet MS', sans-serif;
      display: flex;
      flex-direction: column;
      align-items: center;
      min-height: 100vh;
      padding: 8px;
      gap: 8px;
    }

    /* ── Info bar ── */
    #info-bar {
      display: flex;
      gap: 20px;
      padding: 6px 20px;
      background: #1a1a2e;
      border: 1px solid #2a2a44;
      border-radius: 8px;
      font-size: 13px;
      color: #aaa;
      flex-wrap: wrap;
      justify-content: center;
      width: 100%;
      max-width: 900px;
    }
    #info-bar strong { color: #f0d060; }
    #conn-dot { font-size: 10px; }
    #conn-dot.ok  { color: #44dd66; }
    #conn-dot.err { color: #ff6666; }

    /* ── SVG table ── */
    #poker-table {
      width: min(100%, 900px, calc((100vh - 200px) * 1.5));
      max-width: 900px;
      height: auto;
      display: block;
    }

    /* ── Event log ── */
    #event-log {
      width: 100%;
      max-width: 900px;
      background: #0d0d1a;
      border: 1px solid #1e1e2e;
      border-radius: 6px;
      padding: 6px 10px;
      max-height: 160px;
      overflow-y: auto;
      font-size: 12px;
      color: #667;
      line-height: 1.7;
    }
    #event-log .ev { border-bottom: 1px solid #14141e; padding: 1px 0; }
    #event-log .ev-hand_started    { color: #80c0ff; }
    #event-log .ev-hand_ended      { color: #80ff80; }
    #event-log .ev-street_advanced { color: #ffa060; }
    #event-log .ev-player_action   { color: #cccccc; }
    #event-log .ev-player_seated   { color: #888; }
    #event-log .ev-player_removed  { color: #888; }

    footer {
      font-size: 11px;
      color: #334;
      margin-top: 4px;
    }
    footer a { color: #334; }

    @media (min-width: 900px) {
      body { padding: 12px; }
    }
  </style>
</head>
<body>

  <!-- ── Info bar ── -->
  <div id="info-bar">
    <span><span id="conn-dot" class="err">&#9679;</span> <span id="conn-label">Connecting&hellip;</span></span>
    <span>Hand: <strong id="info-hand">&#8212;</strong></span>
    <span>Street: <strong id="info-street">&#8212;</strong></span>
    <span>Pot: <strong id="info-pot">&#8212;</strong></span>
  </div>

  <!-- ── Poker table SVG (adapted from pkarena0-web, MIT/GPL) ── -->
  <svg id="poker-table" viewBox="0 0 1200 800" xmlns="http://www.w3.org/2000/svg" preserveAspectRatio="xMidYMid meet">
    <defs>
      <radialGradient id="feltGrad" cx="50%" cy="50%" r="50%">
        <stop offset="0%" stop-color="#1a6b3c"/>
        <stop offset="70%" stop-color="#145a30"/>
        <stop offset="100%" stop-color="#0e4825"/>
      </radialGradient>
      <linearGradient id="rimGrad" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stop-color="#5c3a1e"/>
        <stop offset="40%" stop-color="#8b5e3c"/>
        <stop offset="60%" stop-color="#6b4226"/>
        <stop offset="100%" stop-color="#3e2512"/>
      </linearGradient>
      <pattern id="cardBack" width="6" height="6" patternUnits="userSpaceOnUse">
        <rect width="6" height="6" fill="#1a3a6b"/>
        <path d="M0 3 L3 0 L6 3 L3 6 Z" fill="#1e4080" opacity="0.6"/>
      </pattern>
      <filter id="chipShadow" x="-20%" y="-20%" width="140%" height="140%">
        <feDropShadow dx="1" dy="2" stdDeviation="1.5" flood-color="#000" flood-opacity="0.4"/>
      </filter>
      <filter id="cardShadow" x="-10%" y="-10%" width="130%" height="130%">
        <feDropShadow dx="1" dy="2" stdDeviation="2" flood-color="#000" flood-opacity="0.35"/>
      </filter>
      <filter id="potGlow">
        <feGaussianBlur stdDeviation="8" result="blur"/>
        <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
      </filter>
      <filter id="feltNoise">
        <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="4" result="noise"/>
        <feColorMatrix type="saturate" values="0" in="noise" result="grayNoise"/>
        <feBlend in="SourceGraphic" in2="grayNoise" mode="multiply" result="blended"/>
        <feComponentTransfer in="blended"><feFuncA type="linear" slope="1"/></feComponentTransfer>
      </filter>

      <!-- Suit symbols -->
      <symbol id="spade" viewBox="0 0 20 20">
        <path d="M10 2 C10 2 2 10 2 13 C2 16 5 17 7 16 C8 15.5 9 15 10 18 C11 15 12 15.5 13 16 C15 17 18 16 18 13 C18 10 10 2 10 2Z" fill="currentColor"/>
        <rect x="9" y="15" width="2" height="4" rx="0.5" fill="currentColor"/>
      </symbol>
      <symbol id="heart" viewBox="0 0 20 20">
        <path d="M10 18 C10 18 2 12 2 7 C2 4 4.5 2 7 2 C8.5 2 9.5 3 10 4 C10.5 3 11.5 2 13 2 C15.5 2 18 4 18 7 C18 12 10 18 10 18Z" fill="currentColor"/>
      </symbol>
      <symbol id="diamond" viewBox="0 0 20 20">
        <path d="M10 1 L18 10 L10 19 L2 10 Z" fill="currentColor"/>
      </symbol>
      <symbol id="club" viewBox="0 0 20 20">
        <circle cx="10" cy="7" r="4" fill="currentColor"/>
        <circle cx="5.5" cy="12" r="4" fill="currentColor"/>
        <circle cx="14.5" cy="12" r="4" fill="currentColor"/>
        <rect x="9" y="14" width="2" height="5" rx="0.5" fill="currentColor"/>
      </symbol>

      <!-- Face-down card -->
      <symbol id="cardDown" viewBox="0 0 44 62">
        <rect x="0" y="0" width="44" height="62" rx="4" ry="4" fill="#fff" stroke="#999" stroke-width="0.5"/>
        <rect x="2" y="2" width="40" height="58" rx="3" ry="3" fill="url(#cardBack)"/>
        <rect x="2" y="2" width="40" height="58" rx="3" ry="3" fill="none" stroke="#c8a84e" stroke-width="1" opacity="0.5"/>
      </symbol>

      <!-- Chip templates -->
      <symbol id="chipWhite" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r="13" fill="#e8e8e8" stroke="#aaa" stroke-width="1.5"/>
        <circle cx="14" cy="14" r="10" fill="none" stroke="#ccc" stroke-width="1" stroke-dasharray="3 3"/>
      </symbol>
      <symbol id="chipRed" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r="13" fill="#cc2222" stroke="#881111" stroke-width="1.5"/>
        <circle cx="14" cy="14" r="10" fill="none" stroke="#ff6666" stroke-width="1" stroke-dasharray="3 3"/>
      </symbol>
      <symbol id="chipGreen" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r="13" fill="#1a8a3a" stroke="#0e5522" stroke-width="1.5"/>
        <circle cx="14" cy="14" r="10" fill="none" stroke="#44cc66" stroke-width="1" stroke-dasharray="3 3"/>
      </symbol>
      <symbol id="chipBlack" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r="13" fill="#222" stroke="#000" stroke-width="1.5"/>
        <circle cx="14" cy="14" r="10" fill="none" stroke="#555" stroke-width="1" stroke-dasharray="3 3"/>
      </symbol>
      <symbol id="chipBlue" viewBox="0 0 28 28">
        <circle cx="14" cy="14" r="13" fill="#2255bb" stroke="#113388" stroke-width="1.5"/>
        <circle cx="14" cy="14" r="10" fill="none" stroke="#5588ee" stroke-width="1" stroke-dasharray="3 3"/>
      </symbol>
    </defs>

    <!-- Background -->
    <rect width="1200" height="800" fill="#1a1a2e"/>
    <!-- Table structure -->
    <ellipse cx="600" cy="400" rx="360" ry="210" fill="#1a0e08" opacity="0.6"/>
    <ellipse cx="600" cy="395" rx="356" ry="206" fill="url(#rimGrad)" stroke="#2a1508" stroke-width="3"/>
    <ellipse cx="600" cy="395" rx="345" ry="196" fill="#4a3020" stroke="#3a2010" stroke-width="2"/>
    <ellipse cx="600" cy="395" rx="341" ry="193" fill="#5a3828" stroke="#6a4838" stroke-width="1"/>
    <ellipse cx="600" cy="395" rx="323" ry="180" fill="url(#feltGrad)" filter="url(#feltNoise)"/>
    <ellipse cx="600" cy="395" rx="285" ry="157" fill="none" stroke="#1f7a44" stroke-width="2" opacity="0.5"/>
    <text x="600" y="292" text-anchor="middle" font-family="Georgia,serif" font-size="14" fill="#c8a84e" opacity="0.7" letter-spacing="4">NO LIMIT HOLD'EM</text>

    <!-- Hand result overlay -->
    <g id="hand-result-group" visibility="hidden">
      <rect id="hand-result-bg" x="420" y="304" width="360" height="48" rx="14" fill="#0a1e12" opacity="0.92"/>
      <text id="hand-result-text" x="600" y="336" text-anchor="middle"
            font-family="'Trebuchet MS',sans-serif" font-size="26" font-weight="bold" fill="#44ff88" letter-spacing="1"></text>
    </g>

    <!-- Community cards -->
    <g id="board-area" transform="translate(478, 360)" filter="url(#cardShadow)">
      <g id="board-card-0" transform="translate(0,0)"></g>
      <g id="board-card-1" transform="translate(50,0)"></g>
      <g id="board-card-2" transform="translate(100,0)"></g>
      <g id="board-card-3" transform="translate(150,0)"></g>
      <g id="board-card-4" transform="translate(200,0)"></g>
    </g>

    <!-- Pot -->
    <g transform="translate(572,432)" filter="url(#chipShadow)">
      <use href="#chipBlack" x="0" y="0" width="24" height="24"/>
      <use href="#chipBlack" x="2" y="-4" width="24" height="24"/>
      <use href="#chipRed"   x="18" y="2" width="24" height="24"/>
      <use href="#chipGreen" x="36" y="-1" width="24" height="24"/>
      <use href="#chipBlue"  x="54" y="1" width="24" height="24"/>
    </g>
    <text id="pot-amount" x="600" y="476" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#ffdd66" font-weight="bold">POT: $0</text>

    <!-- SEAT 0 — bottom center -->
    <g id="seat-0-group" transform="translate(600,680)">
      <ellipse id="seat-0-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-0-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-0-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-0-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-0-card-0" transform="translate(-32,-90)" filter="url(#cardShadow)"></g>
      <g id="seat-0-card-1" transform="translate(-6,-90)" filter="url(#cardShadow)"></g>
      <text id="seat-0-bet" x="0" y="56" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-0-action" visibility="hidden">
        <rect id="seat-0-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-0-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-0-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-0-badge-rect"/>
        <text id="seat-0-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-0-btn-d" visibility="hidden" transform="translate(82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-0-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-0-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 1 — bottom left -->
    <g id="seat-1-group" transform="translate(280,640)">
      <ellipse id="seat-1-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-1-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-1-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-1-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-1-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-1-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-1-bet" x="78" y="-36" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-1-action" visibility="hidden">
        <rect id="seat-1-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-1-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-1-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-1-badge-rect"/>
        <text id="seat-1-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-1-btn-d" visibility="hidden" transform="translate(82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-1-btn-sb" visibility="hidden" transform="translate(82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-1-btn-bb" visibility="hidden" transform="translate(82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 2 — left -->
    <g id="seat-2-group" transform="translate(140,480)">
      <ellipse id="seat-2-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-2-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-2-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-2-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-2-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-2-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-2-bet" x="80" y="-32" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-2-action" visibility="hidden">
        <rect id="seat-2-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-2-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-2-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-2-badge-rect"/>
        <text id="seat-2-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-2-btn-d" visibility="hidden" transform="translate(82,-44)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-2-btn-sb" visibility="hidden" transform="translate(82,-44)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-2-btn-bb" visibility="hidden" transform="translate(82,-44)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 3 — upper left -->
    <g id="seat-3-group" transform="translate(170,260)">
      <ellipse id="seat-3-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-3-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-3-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-3-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-3-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-3-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-3-bet" x="80" y="-32" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-3-action" visibility="hidden">
        <rect id="seat-3-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-3-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-3-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-3-badge-rect"/>
        <text id="seat-3-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-3-btn-d" visibility="hidden" transform="translate(82,-44)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-3-btn-sb" visibility="hidden" transform="translate(-82,-44)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-3-btn-bb" visibility="hidden" transform="translate(-82,-44)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 4 — top left -->
    <g id="seat-4-group" transform="translate(340,140)">
      <ellipse id="seat-4-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-4-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-4-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-4-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-4-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-4-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-4-bet" x="80" y="-34" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-4-action" visibility="hidden">
        <rect id="seat-4-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-4-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-4-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-4-badge-rect"/>
        <text id="seat-4-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-4-btn-d" visibility="hidden" transform="translate(82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-4-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-4-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 5 — top center -->
    <g id="seat-5-group" transform="translate(600,108)">
      <ellipse id="seat-5-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-5-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-5-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-5-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-5-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-5-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-5-bet" x="80" y="-34" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-5-action" visibility="hidden">
        <rect id="seat-5-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-5-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-5-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-5-badge-rect"/>
        <text id="seat-5-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-5-btn-d" visibility="hidden" transform="translate(82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-5-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-5-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 6 — top right -->
    <g id="seat-6-group" transform="translate(860,140)">
      <ellipse id="seat-6-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-6-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-6-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-6-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-6-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-6-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-6-bet" x="-80" y="-32" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-6-action" visibility="hidden">
        <rect id="seat-6-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-6-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-6-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-6-badge-rect"/>
        <text id="seat-6-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-6-btn-d" visibility="hidden" transform="translate(-82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-6-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-6-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 7 — upper right -->
    <g id="seat-7-group" transform="translate(1030,260)">
      <ellipse id="seat-7-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-7-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-7-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-7-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-7-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-7-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-7-bet" x="-80" y="-32" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-7-action" visibility="hidden">
        <rect id="seat-7-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-7-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-7-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-7-badge-rect"/>
        <text id="seat-7-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-7-btn-d" visibility="hidden" transform="translate(-82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-7-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-7-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>

    <!-- SEAT 8 — bottom right -->
    <g id="seat-8-group" transform="translate(920,640)">
      <ellipse id="seat-8-highlight" cx="0" cy="-10" rx="78" ry="40" fill="#c8a84e" opacity="0"/>
      <rect id="seat-8-plate" x="-72" y="-28" width="144" height="44" rx="14" fill="#1a1a2e" stroke="#4a90d9" stroke-width="1.2" opacity="0.9"/>
      <text id="seat-8-name" x="0" y="-12" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="14" fill="#7ab8f5" font-weight="bold">&#8212;</text>
      <text id="seat-8-chips" x="0" y="6" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="12" fill="#aaa">$&#8212;</text>
      <g id="seat-8-card-0" transform="translate(-28,-85)" filter="url(#cardShadow)"></g>
      <g id="seat-8-card-1" transform="translate(-8,-85)" filter="url(#cardShadow)"></g>
      <text id="seat-8-bet" x="-80" y="-46" font-family="'Trebuchet MS',sans-serif" font-size="10" fill="#ffdd66" visibility="hidden"></text>
      <g id="seat-8-action" visibility="hidden">
        <rect id="seat-8-action-bg" x="-68" y="20" width="136" height="26" rx="13" fill="#333"/>
        <text id="seat-8-action-text" x="0" y="37" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="13" font-weight="bold" fill="#fff"></text>
      </g>
      <g id="seat-8-badge" visibility="hidden">
        <rect x="-30" y="52" width="60" height="16" rx="8" id="seat-8-badge-rect"/>
        <text id="seat-8-badge-text" x="0" y="64" text-anchor="middle" font-family="'Trebuchet MS',sans-serif" font-size="9" font-weight="bold"></text>
      </g>
      <g id="seat-8-btn-d" visibility="hidden" transform="translate(-82,-46)">
        <circle r="10" fill="#ffffcc" stroke="#c8a84e" stroke-width="1.5"/>
        <text y="4" text-anchor="middle" font-size="8" font-weight="bold" fill="#333">D</text>
      </g>
      <g id="seat-8-btn-sb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#335588" stroke="#5588cc" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#aaddff">SB</text>
      </g>
      <g id="seat-8-btn-bb" visibility="hidden" transform="translate(-82,-46)">
        <circle r="9" fill="#885533" stroke="#cc8844" stroke-width="1"/>
        <text y="3" text-anchor="middle" font-size="7" font-weight="bold" fill="#ffcc88">BB</text>
      </g>
    </g>
  </svg>

  <!-- ── Event log ── -->
  <div id="event-log"></div>

  <footer>
    <a href="https://github.com/ImperialBower/pkdealer">ImperialBower/pkdealer</a>
  </footer>

  <script>
    'use strict';

    const SVG_NS = 'http://www.w3.org/2000/svg';

    // Per-seat persisted action labels (cleared on hand_started).
    const lastActions = {};

    // ── DOM helpers ───────────────────────────────────────────────────────────

    function clearChildren(el) {
      while (el && el.firstChild) el.removeChild(el.firstChild);
    }
    function setText(id, text) {
      const el = document.getElementById(id);
      if (el) el.textContent = text;
    }
    function setVisibility(id, v) {
      const el = document.getElementById(id);
      if (el) el.setAttribute('visibility', v);
    }

    // ── State mapping ─────────────────────────────────────────────────────────

    // Maps the lowercase underscore state strings from SpectatorSnapshot to
    // the short tokens that updateSeat uses for badge and opacity decisions.
    const STATE_MAP = {
      out:       'Out',
      folded:    'Fold',
      all_in:    'AllIn',
      yet_to_act:'Active',
      blind:     'Active',
      ready:     'Ready',
      checked:   'Active',
      called:    'Active',
      bet:       'Active',
      raised:    'Active',
      unspecified: 'Ready',
    };

    function mapState(s) { return STATE_MAP[s] ?? 'Active'; }

    // ── Action callout helpers ────────────────────────────────────────────────

    function actionColors(label) {
      const l = (label ?? '').toLowerCase();
      if (l.startsWith('fold'))                          return { fill: '#882222', color: '#ff8888' };
      if (l.startsWith('check'))                         return { fill: '#1a5c2e', color: '#55dd88' };
      if (l.startsWith('call'))                          return { fill: '#1a3a6e', color: '#66aaff' };
      if (l.startsWith('bet') || l.startsWith('raise'))  return { fill: '#5c4400', color: '#ffcc44' };
      if (l.startsWith('all'))                           return { fill: '#884400', color: '#ff9922' };
      return { fill: '#333', color: '#fff' };
    }

    function setActionLabel(seat, label) {
      lastActions[seat] = label ?? null;
      const actionGrp = document.getElementById('seat-' + seat + '-action');
      const actionBg  = document.getElementById('seat-' + seat + '-action-bg');
      const actionTxt = document.getElementById('seat-' + seat + '-action-text');
      if (!actionGrp || !actionBg || !actionTxt) return;
      if (!label) { actionGrp.setAttribute('visibility', 'hidden'); return; }
      const { fill, color } = actionColors(label);
      actionBg.setAttribute('fill', fill);
      actionTxt.setAttribute('fill', color);
      actionTxt.textContent = label;
      actionGrp.setAttribute('visibility', 'visible');
    }

    function clearAllActions() {
      for (let i = 0; i < 9; i++) {
        lastActions[i] = null;
        const g = document.getElementById('seat-' + i + '-action');
        if (g) g.setAttribute('visibility', 'hidden');
      }
    }

    // ── Card rendering ────────────────────────────────────────────────────────

    function renderCard(groupId, cardStr, isBoardSlot) {
      const g = document.getElementById(groupId);
      if (!g) return;
      clearChildren(g);

      if (!cardStr) {
        // Empty board slot: nothing to draw.
        return;
      }

      if (cardStr === '__') {
        const u = document.createElementNS(SVG_NS, 'use');
        u.setAttribute('href', '#cardDown');
        u.setAttribute('width', '44');
        u.setAttribute('height', '62');
        g.appendChild(u);
        return;
      }

      // Face-up card: two-char string like "Ah", "Kd", "Ts".
      const rankChar  = cardStr[0];
      const suitChar  = cardStr[1];
      const rankLabel = rankChar === 'T' ? '10' : rankChar;
      const suitId    = { s: 'spade', h: 'heart', d: 'diamond', c: 'club' }[suitChar] ?? 'spade';
      const fill      = (suitChar === 'h' || suitChar === 'd') ? '#cc1111' : '#222';

      const rect = document.createElementNS(SVG_NS, 'rect');
      rect.setAttribute('width', '44');
      rect.setAttribute('height', '62');
      rect.setAttribute('rx', '4');
      rect.setAttribute('fill', '#fff');
      rect.setAttribute('stroke', '#ccc');
      rect.setAttribute('stroke-width', '0.5');

      const rankText = document.createElementNS(SVG_NS, 'text');
      rankText.setAttribute('x', '3');
      rankText.setAttribute('y', '16');
      rankText.setAttribute('font-family', 'Georgia,serif');
      rankText.setAttribute('font-size', rankLabel === '10' ? '12' : '14');
      rankText.setAttribute('font-weight', 'bold');
      rankText.setAttribute('fill', fill);
      rankText.textContent = rankLabel;

      const suitUse = document.createElementNS(SVG_NS, 'use');
      suitUse.setAttribute('href', '#' + suitId);
      suitUse.setAttribute('x', '12');
      suitUse.setAttribute('y', '24');
      suitUse.setAttribute('width', '20');
      suitUse.setAttribute('height', '20');
      suitUse.setAttribute('color', fill);

      g.appendChild(rect);
      g.appendChild(rankText);
      g.appendChild(suitUse);
    }

    function clearCards(seatIdx) {
      clearChildren(document.getElementById('seat-' + seatIdx + '-card-0'));
      clearChildren(document.getElementById('seat-' + seatIdx + '-card-1'));
    }

    // ── Seat rendering ────────────────────────────────────────────────────────

    // `player` shape: { name, chips, bet, state, cards, is_dealer, is_sb, is_bb }
    // `renderData`  : { next_to_act, hand_in_progress, ... }
    function updateSeat(seatIdx, player, renderData) {
      const grp = document.getElementById('seat-' + seatIdx + '-group');
      if (!grp) return;

      if (!player || player.state === 'Out') {
        grp.setAttribute('opacity', '0.25');
        clearCards(seatIdx);
        return;
      }
      grp.setAttribute('opacity', '1');

      setText('seat-' + seatIdx + '-name',  player.name  || '—');
      setText('seat-' + seatIdx + '-chips', '$' + (player.chips ?? 0).toLocaleString());

      // Hole cards: show face-up if known, face-down for active players, none otherwise.
      const cards    = player.cards ?? [];
      const isActive = player.state !== 'Fold' && player.state !== 'Out';
      const showDown = isActive && renderData.hand_in_progress;
      renderCard('seat-' + seatIdx + '-card-0', cards[0] ?? (showDown ? '__' : null), false);
      renderCard('seat-' + seatIdx + '-card-1', cards[1] ?? (showDown ? '__' : null), false);

      // Current bet
      const betEl = document.getElementById('seat-' + seatIdx + '-bet');
      if (betEl) {
        if (player.bet > 0) {
          betEl.textContent = '$' + player.bet.toLocaleString();
          betEl.setAttribute('visibility', 'visible');
        } else {
          betEl.setAttribute('visibility', 'hidden');
        }
      }

      // FOLD / ALL-IN status badges
      const badgeGrp  = document.getElementById('seat-' + seatIdx + '-badge');
      const badgeText = document.getElementById('seat-' + seatIdx + '-badge-text');
      const badgeRect = document.getElementById('seat-' + seatIdx + '-badge-rect');
      if (badgeGrp && badgeText && badgeRect) {
        if (player.state === 'Fold') {
          badgeGrp.setAttribute('visibility', 'visible');
          badgeText.textContent = 'FOLD';
          badgeText.setAttribute('fill', '#ff8888');
          badgeRect.setAttribute('fill', '#882222');
          badgeRect.setAttribute('opacity', '0.7');
        } else if (player.state === 'AllIn') {
          badgeGrp.setAttribute('visibility', 'visible');
          badgeText.textContent = 'ALL-IN';
          badgeText.setAttribute('fill', '#ffffff');
          badgeRect.setAttribute('fill', '#cc8800');
          badgeRect.setAttribute('opacity', '0.8');
        } else {
          badgeGrp.setAttribute('visibility', 'hidden');
        }
      }

      // D / SB / BB position buttons (not populated by SSE yet; hidden by default)
      setVisibility('seat-' + seatIdx + '-btn-d',  player.is_dealer ? 'visible' : 'hidden');
      setVisibility('seat-' + seatIdx + '-btn-sb', player.is_sb     ? 'visible' : 'hidden');
      setVisibility('seat-' + seatIdx + '-btn-bb', player.is_bb     ? 'visible' : 'hidden');

      // Next-to-act highlight + plate border
      const isNext = renderData.hand_in_progress && (seatIdx === renderData.next_to_act);
      const hlEl   = document.getElementById('seat-' + seatIdx + '-highlight');
      if (hlEl) hlEl.setAttribute('opacity', isNext ? '0.12' : '0');

      const plateEl = document.getElementById('seat-' + seatIdx + '-plate');
      if (plateEl) {
        if (isNext) {
          plateEl.setAttribute('stroke', '#c8a84e');
        } else if (player.state === 'Fold' || player.state === 'Out') {
          plateEl.setAttribute('stroke', '#333');
        } else {
          plateEl.setAttribute('stroke', '#4a90d9');
        }
      }

      // Restore persisted action callout (survives re-renders)
      const actionGrp = document.getElementById('seat-' + seatIdx + '-action');
      const actionBg  = document.getElementById('seat-' + seatIdx + '-action-bg');
      const actionTxt = document.getElementById('seat-' + seatIdx + '-action-text');
      if (actionGrp && actionBg && actionTxt) {
        const label = lastActions[seatIdx];
        if (label) {
          const { fill, color } = actionColors(label);
          actionBg.setAttribute('fill', fill);
          actionTxt.setAttribute('fill', color);
          actionTxt.textContent = label;
          actionGrp.setAttribute('visibility', 'visible');
        } else {
          actionGrp.setAttribute('visibility', 'hidden');
        }
      }
    }

    // ── Table rendering ───────────────────────────────────────────────────────

    // `renderData` is the SpectatorSnapshot JSON with an added `seat_map`
    // (object keyed by seat_number → player data).
    function renderTableVisuals(renderData) {
      // Pot and board cards
      setText('pot-amount', 'POT: $' + (renderData.pot ?? 0).toLocaleString());
      const board = renderData.board ? renderData.board.split(' ').filter(Boolean) : [];
      for (let i = 0; i < 5; i++) {
        renderCard('board-card-' + i, board[i] ?? null, true);
      }

      // All nine seat positions
      for (let i = 0; i < 9; i++) {
        updateSeat(i, renderData.seat_map[i] ?? null, renderData);
      }
    }

    function clearTable() {
      for (let i = 0; i < 5; i++) clearChildren(document.getElementById('board-card-' + i));
      setText('pot-amount', 'POT: $0');
      const hrg = document.getElementById('hand-result-group');
      if (hrg) hrg.setAttribute('visibility', 'hidden');
      clearAllActions();
      for (let i = 0; i < 9; i++) {
        const p = 'seat-' + i + '-';
        const h  = document.getElementById(p + 'highlight');
        const b  = document.getElementById(p + 'bet');
        const bx = document.getElementById(p + 'badge');
        const d  = document.getElementById(p + 'btn-d');
        const sb = document.getElementById(p + 'btn-sb');
        const bb = document.getElementById(p + 'btn-bb');
        if (h)  h.setAttribute('opacity', '0');
        if (b)  b.setAttribute('visibility', 'hidden');
        if (bx) bx.setAttribute('visibility', 'hidden');
        if (d)  d.setAttribute('visibility', 'hidden');
        if (sb) sb.setAttribute('visibility', 'hidden');
        if (bb) bb.setAttribute('visibility', 'hidden');
        clearCards(i);
      }
    }

    // ── Text scaling ──────────────────────────────────────────────────────────

    function scaleTableText() {
      const svg   = document.getElementById('poker-table');
      const w     = svg.getBoundingClientRect().width;
      if (!w) return;
      const scale = w / 1200;

      const nameSize   = Math.min(26, Math.max(14, Math.round(10 / scale)));
      const chipsSize  = Math.min(21, Math.max(12, Math.round( 8 / scale)));
      const actionSize = Math.min(21, Math.max(13, Math.round(10 / scale)));
      const badgeSize  = Math.min(15, Math.max( 9, Math.round( 7 / scale)));
      const potSize    = Math.min(23, Math.max(14, Math.round(10 / scale)));
      const chipsY     = nameSize > 16 ? 10 : 6;

      for (let i = 0; i <= 8; i++) {
        const name   = document.getElementById('seat-' + i + '-name');
        const chips  = document.getElementById('seat-' + i + '-chips');
        const action = document.getElementById('seat-' + i + '-action-text');
        const badge  = document.getElementById('seat-' + i + '-badge-text');
        if (name)   name.setAttribute('font-size', nameSize);
        if (chips)  { chips.setAttribute('font-size', chipsSize); chips.setAttribute('y', chipsY); }
        if (action) action.setAttribute('font-size', actionSize);
        if (badge)  badge.setAttribute('font-size', badgeSize);
      }
      const potEl = document.getElementById('pot-amount');
      if (potEl) potEl.setAttribute('font-size', potSize);
    }

    new ResizeObserver(scaleTableText).observe(document.getElementById('poker-table'));
    setTimeout(scaleTableText, 0);

    // ── SSE → render pipeline ─────────────────────────────────────────────────

    // Converts a SpectatorSnapshot to the shape renderTableVisuals expects.
    function buildRenderData(status) {
      const seatMap = {};
      for (const s of status.seats ?? []) {
        seatMap[s.seat_number] = {
          name:      s.player_name,
          chips:     s.chips,
          bet:       0,           // not exposed via SSE yet
          state:     mapState(s.state),
          cards:     (s.cards && s.cards.length > 0) ? s.cards : [],
          is_dealer: false,       // not exposed via SSE yet
          is_sb:     false,
          is_bb:     false,
        };
      }
      return {
        pot:            status.pot ?? 0,
        board:          status.board ?? '',
        next_to_act:    status.next_to_act ?? 0,
        hand_in_progress: !!status.hand_in_progress,
        current_street: status.current_street ?? '',
        seat_map:       seatMap,
      };
    }

    // Labels extracted from description "Seat N: Verb".
    const ACTION_LABELS = {
      Fold:  'folds', Call:  'calls', Check: 'checks',
      Bet:   'bets',  Raise: 'raises', AllIn: 'all-in',
    };

    let handNumber = 0;

    function applyEvent(ev) {
      // ── Info bar ──────────────────────────────────────────────────────────
      if (ev.status) {
        const s = ev.status;
        if (ev.event_type === 'hand_started') handNumber++;
        setText('info-hand',   handNumber > 0 ? String(handNumber) : '—');
        setText('info-street', s.current_street || '—');
        setText('info-pot',    s.pot > 0 ? '$' + s.pot.toLocaleString() : '—');
      }

      // ── Event-specific behaviour ──────────────────────────────────────────
      if (ev.event_type === 'hand_started') clearAllActions();

      if (ev.event_type === 'player_action') {
        const m = (ev.description ?? '').match(/^Seat (\d+): (\w+)/);
        if (m) {
          const seat  = parseInt(m[1], 10);
          const label = ACTION_LABELS[m[2]] ?? m[2].toLowerCase();
          setActionLabel(seat, label);
        }
      }

      // ── Table visuals ─────────────────────────────────────────────────────
      if (ev.status) renderTableVisuals(buildRenderData(ev.status));

      // ── Event log ─────────────────────────────────────────────────────────
      const log  = document.getElementById('event-log');
      const div  = document.createElement('div');
      div.className = 'ev ev-' + (ev.event_type ?? 'unknown');
      div.textContent = ev.description || ev.event_type || '?';
      log.insertBefore(div, log.firstChild);
      while (log.children.length > 40) log.removeChild(log.lastChild);
    }

    // ── SSE connection ────────────────────────────────────────────────────────

    (function connect() {
      const es = new EventSource('/events');

      es.onopen = function() {
        const dot   = document.getElementById('conn-dot');
        const label = document.getElementById('conn-label');
        if (dot)   { dot.className = 'ok'; }
        if (label) { label.textContent = 'Connected'; }
      };

      es.onerror = function() {
        const dot   = document.getElementById('conn-dot');
        const label = document.getElementById('conn-label');
        if (dot)   { dot.className = 'err'; }
        if (label) { label.textContent = 'Reconnecting…'; }
      };

      es.onmessage = function(e) {
        try {
          applyEvent(JSON.parse(e.data));
        } catch (_) { /* ignore malformed events */ }
      };
    })();
  </script>
</body>
</html>"##;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Shared state for axum route handlers.
pub(crate) struct WebState {
    pub event_tx: broadcast::Sender<TableEvent>,
}

// ── Browser-facing types ──────────────────────────────────────────────────────

/// A [`TableEvent`] translated to a browser-safe JSON shape.
#[derive(Serialize)]
pub struct SpectatorEvent {
    /// Human-readable event type string (e.g. `"hand_started"`).
    pub event_type: String,
    /// Human-readable description of what happened.
    pub description: String,
    /// Unix millisecond timestamp from the service.
    pub timestamp: u64,
    /// Current table state after the event, or `None` if not available.
    pub status: Option<SpectatorSnapshot>,
}

/// A snapshot of the table for the spectator page.
#[derive(Serialize, Clone)]
pub struct SpectatorSnapshot {
    pub seats: Vec<SpectatorSeat>,
    pub board: String,
    pub pot: u32,
    pub next_to_act: u32,
    pub current_street: String,
    pub hand_in_progress: bool,
}

/// One seat's information for the spectator.
///
/// `cards` is populated only when the broadcast channel carries a spectator-token
/// snapshot (`CardVisibility::Spectator`); it is an empty `Vec` for hidden-card events.
#[derive(Serialize, Clone)]
pub struct SpectatorSeat {
    pub seat_number: u32,
    pub player_name: String,
    pub chips: u32,
    pub state: String,
    pub cards: Vec<String>,
}

// ── Mapping helpers ───────────────────────────────────────────────────────────

fn event_type_to_str(raw: i32) -> &'static str {
    match EventType::try_from(raw).unwrap_or(EventType::Unspecified) {
        EventType::Unspecified => "unspecified",
        EventType::PlayerSeated => "player_seated",
        EventType::PlayerRemoved => "player_removed",
        EventType::HandStarted => "hand_started",
        EventType::PlayerAction => "player_action",
        EventType::StreetAdvanced => "street_advanced",
        EventType::HandEnded => "hand_ended",
    }
}

fn street_to_str(raw: i32) -> &'static str {
    match Street::try_from(raw).unwrap_or(Street::Unspecified) {
        Street::Unspecified => "unspecified",
        Street::Preflop => "preflop",
        Street::Flop => "flop",
        Street::Turn => "turn",
        Street::River => "river",
    }
}

fn player_state_to_str(raw: i32) -> &'static str {
    match ProtoPlayerState::try_from(raw).unwrap_or(ProtoPlayerState::Unspecified) {
        ProtoPlayerState::Unspecified => "unspecified",
        ProtoPlayerState::Ready => "ready",
        ProtoPlayerState::YetToAct => "yet_to_act",
        ProtoPlayerState::Checked => "checked",
        ProtoPlayerState::Called => "called",
        ProtoPlayerState::Bet => "bet",
        ProtoPlayerState::Raised => "raised",
        ProtoPlayerState::AllIn => "all_in",
        ProtoPlayerState::Folded => "folded",
        ProtoPlayerState::Out => "out",
        ProtoPlayerState::Blind => "blind",
    }
}

fn table_status_to_snapshot(status: &TableStatus) -> SpectatorSnapshot {
    SpectatorSnapshot {
        seats: status
            .seats
            .iter()
            .map(|s| SpectatorSeat {
                seat_number: s.seat_number,
                player_name: s.player_name.clone(),
                chips: s.chips,
                state: player_state_to_str(s.state).to_owned(),
                cards: if s.cards.is_empty() {
                    vec![]
                } else {
                    s.cards.split_whitespace().map(str::to_owned).collect()
                },
            })
            .collect(),
        board: status.board.clone(),
        pot: status.pot,
        next_to_act: status.next_to_act,
        current_street: street_to_str(status.current_street).to_owned(),
        hand_in_progress: status.hand_in_progress,
    }
}

// ── SpectatorEvent construction ───────────────────────────────────────────────

impl SpectatorEvent {
    fn from_proto(event: &TableEvent) -> Self {
        SpectatorEvent {
            event_type: event_type_to_str(event.event_type).to_owned(),
            description: event.description.clone(),
            timestamp: event.timestamp,
            status: event.current_status.as_ref().map(table_status_to_snapshot),
        }
    }
}

// ── Route handlers ────────────────────────────────────────────────────────────

async fn handle_index() -> Html<&'static str> {
    Html(SPECTATOR_HTML)
}

/// SSE endpoint: streams [`TableEvent`]s as newline-delimited JSON to browser clients.
///
/// Lagged events are silently skipped — spectators are observers and a gap in the
/// log is acceptable.
async fn handle_sse(
    State(state): State<Arc<WebState>>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let mut broadcast_rx = state.event_tx.subscribe();
    let (mpsc_tx, mpsc_rx) = mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    let se = SpectatorEvent::from_proto(&event);
                    let data = serde_json::to_string(&se).unwrap_or_default();
                    if mpsc_tx.send(Ok(Event::default().data(data))).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            }
        }
    });

    Sse::new(ReceiverStream::new(mpsc_rx)).keep_alive(KeepAlive::default())
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Starts the Axum HTTP server on the provided pre-bound listener.
///
/// # Errors
///
/// Returns `Err` if the underlying `axum::serve` call fails.
pub async fn serve(
    listener: TcpListener,
    event_tx: broadcast::Sender<TableEvent>,
) -> Result<(), std::io::Error> {
    let state = Arc::new(WebState { event_tx });
    let app = Router::new()
        .route("/", get(handle_index))
        .route("/events", get(handle_sse))
        .with_state(state);
    axum::serve(listener, app).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use pkdealer_proto::dealer::SeatInfo;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // ── Mapping helpers ───────────────────────────────────────────────────────

    #[test]
    fn event_type_to_str_all_variants() {
        assert_eq!(
            event_type_to_str(EventType::HandStarted as i32),
            "hand_started"
        );
        assert_eq!(event_type_to_str(EventType::HandEnded as i32), "hand_ended");
        assert_eq!(
            event_type_to_str(EventType::PlayerAction as i32),
            "player_action"
        );
        assert_eq!(
            event_type_to_str(EventType::PlayerSeated as i32),
            "player_seated"
        );
        assert_eq!(
            event_type_to_str(EventType::StreetAdvanced as i32),
            "street_advanced"
        );
        assert_eq!(event_type_to_str(999), "unspecified");
    }

    #[test]
    fn street_to_str_all_variants() {
        assert_eq!(street_to_str(Street::Preflop as i32), "preflop");
        assert_eq!(street_to_str(Street::Flop as i32), "flop");
        assert_eq!(street_to_str(Street::Turn as i32), "turn");
        assert_eq!(street_to_str(Street::River as i32), "river");
        assert_eq!(street_to_str(999), "unspecified");
    }

    #[test]
    fn player_state_to_str_all_variants() {
        assert_eq!(
            player_state_to_str(ProtoPlayerState::YetToAct as i32),
            "yet_to_act"
        );
        assert_eq!(
            player_state_to_str(ProtoPlayerState::Folded as i32),
            "folded"
        );
        assert_eq!(
            player_state_to_str(ProtoPlayerState::AllIn as i32),
            "all_in"
        );
        assert_eq!(player_state_to_str(ProtoPlayerState::Blind as i32), "blind");
        assert_eq!(player_state_to_str(999), "unspecified");
    }

    // ── table_status_to_snapshot ──────────────────────────────────────────────

    #[test]
    fn table_status_to_snapshot_maps_seat_state() {
        let status = TableStatus {
            seats: vec![SeatInfo {
                seat_number: 2,
                player_name: "Alice".to_owned(),
                chips: 900,
                cards: String::new(),
                state: ProtoPlayerState::YetToAct as i32,
            }],
            board: "Ah Kd".to_owned(),
            pot: 150,
            current_street: Street::Flop as i32,
            ..Default::default()
        };
        let snap = table_status_to_snapshot(&status);
        assert_eq!(snap.seats.len(), 1);
        assert_eq!(snap.seats[0].player_name, "Alice");
        assert_eq!(snap.seats[0].chips, 900);
        assert_eq!(snap.seats[0].state, "yet_to_act");
        assert!(snap.seats[0].cards.is_empty());
        assert_eq!(snap.current_street, "flop");
        assert_eq!(snap.pot, 150);
        assert_eq!(snap.board, "Ah Kd");
    }

    #[test]
    fn table_status_to_snapshot_parses_hole_cards() {
        let status = TableStatus {
            seats: vec![SeatInfo {
                seat_number: 0,
                player_name: "Bob".to_owned(),
                chips: 500,
                cards: "Ah Kd".to_owned(),
                state: ProtoPlayerState::YetToAct as i32,
            }],
            ..Default::default()
        };
        let snap = table_status_to_snapshot(&status);
        assert_eq!(snap.seats[0].cards, vec!["Ah", "Kd"]);
    }

    #[test]
    fn table_status_to_snapshot_empty_seats() {
        let status = TableStatus::default();
        let snap = table_status_to_snapshot(&status);
        assert!(snap.seats.is_empty());
        assert_eq!(snap.current_street, "unspecified");
        assert_eq!(snap.pot, 0);
    }

    // ── SpectatorEvent ────────────────────────────────────────────────────────

    #[test]
    fn spectator_event_from_proto_without_status() {
        let event = TableEvent {
            timestamp: 42,
            event_type: EventType::PlayerSeated as i32,
            description: "Alice seated".to_owned(),
            current_status: None,
        };
        let se = SpectatorEvent::from_proto(&event);
        assert_eq!(se.event_type, "player_seated");
        assert_eq!(se.description, "Alice seated");
        assert_eq!(se.timestamp, 42);
        assert!(se.status.is_none());
    }

    #[test]
    fn spectator_event_from_proto_with_status() {
        let event = TableEvent {
            timestamp: 100,
            event_type: EventType::HandStarted as i32,
            description: "Hand started".to_owned(),
            current_status: Some(TableStatus {
                pot: 150,
                current_street: Street::Preflop as i32,
                hand_in_progress: true,
                ..Default::default()
            }),
        };
        let se = SpectatorEvent::from_proto(&event);
        assert_eq!(se.event_type, "hand_started");
        let snap = se.status.unwrap();
        assert_eq!(snap.pot, 150);
        assert_eq!(snap.current_street, "preflop");
        assert!(snap.hand_in_progress);
    }

    #[test]
    fn spectator_event_serializes_to_json() {
        let ev = SpectatorEvent {
            event_type: "hand_started".to_owned(),
            description: "Hand started".to_owned(),
            timestamp: 1,
            status: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("hand_started"));
        assert!(json.contains("Hand started"));
        assert!(json.contains("timestamp"));
    }

    // ── HTTP server ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn serve_index_returns_200() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, _) = broadcast::channel::<TableEvent>(4);

        tokio::spawn(serve(listener, tx));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let resp = String::from_utf8_lossy(&buf);
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "expected 200, got: {resp}"
        );
        assert!(resp.contains("PKDealer"), "expected 'PKDealer' in body");
        Ok(())
    }
}
