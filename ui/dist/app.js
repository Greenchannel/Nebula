/* =========================================================================
   Nebula (星云) — single-page responsive client
   -------------------------------------------------------------------------
   One DOM tree for phone / tablet / desktop. Layout is pure CSS (container
   queries); this file never rebuilds the shell or the composer when the
   viewport changes, so drafts, recordings and the active room survive any
   rotation or window resize.
   ========================================================================= */
(() => {
  'use strict';

  // ------------------------------------------------------------------
  // Utilities
  // ------------------------------------------------------------------
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  const esc = (value) => String(value ?? '').replace(/[&<>"']/g, (ch) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[ch]));

  const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
  const safeColor = (c) => (/^#[0-9a-fA-F]{3,8}$/.test(String(c || '')) ? c : '#6366f1');
  const initialOf = (name) => {
    const s = String(name || '?').trim();
    return s ? Array.from(s)[0].toUpperCase() : '?';
  };
  const parseMeta = (raw) => {
    if (!raw) return {};
    if (typeof raw === 'object') return raw;
    try { return JSON.parse(raw) || {}; } catch { return {}; }
  };
  const thumbSrc = (b64) => {
    if (!b64) return '';
    const s = String(b64);
    return s.startsWith('data:') ? s : `data:image/jpeg;base64,${s}`;
  };
  const fmtBytes = (n) => {
    const v = Number(n) || 0;
    if (v < 1024) return `${v} B`;
    if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
    if (v < 1024 * 1024 * 1024) return `${(v / 1024 / 1024).toFixed(1)} MB`;
    return `${(v / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };
  const fmtSoundDur = (ms) => {
    const s = Math.max(0, Math.round((Number(ms) || 0) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  };
  const fmtClock = (ms) => fmtSoundDur(ms);
  const fmtTime = (ms) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const fmtDay = (ms) => {
    const day = startOfDay(new Date(ms));
    const today = startOfDay(new Date());
    const diff = Math.round((today - day) / 86400000);
    if (diff === 0) return '今天';
    if (diff === 1) return '昨天';
    const d = new Date(ms);
    return `${d.getMonth() + 1}月${d.getDate()}日`;
  };
  const fmtWhen = (ms) => {
    const diff = Date.now() - ms;
    if (diff < 0) return '刚刚';
    if (diff < 60e3) return '刚刚';
    if (diff < 3600e3) return `${Math.floor(diff / 60e3)} 分钟前`;
    if (diff < 86400e3) return `${Math.floor(diff / 3600e3)} 小时前`;
    const d = new Date(ms);
    return `${d.getMonth() + 1}/${d.getDate()} ${fmtTime(ms)}`;
  };

  const DEFAULT_WAVE = [22, 48, 76, 55, 92, 40, 66, 84, 52, 73, 36, 61, 88, 49, 70, 32];

  // ------------------------------------------------------------------
  // App state
  // ------------------------------------------------------------------
  const state = {
    identity: null,
    node: null,
    peers: [],
    conversations: [],
    moments: [],
    messages: new Map(),      // conversation_id -> MessageRecord[]
    media: [],
    pendingBlobs: [],
    blobStats: { count: 0, bytes: 0 },
    presentBlobs: new Set(),
    blobRev: 1,
    currentTab: 'chat',
    currentConversation: 'lobby',
    unread: new Map(),
    drafts: new Map(),
    sim: 'auto',
    ws: null,
    wsConnected: false,
    reconnectDelay: 1000,
    refreshTimer: null,
    pendingHighlight: null,
    sending: false,
    momentDraft: { items: [] },
    momentRecording: null,
    momentModal: null,
    voiceGesture: null,
    audio: { el: null, id: null, btn: null },
    recordingTimer: null,
  };

  const els = {};
  const byId = (id) => document.getElementById(id);

  function cacheElements() {
    Object.assign(els, {
      app: byId('nebula-app'),
      simFrame: byId('sim-frame'),
      simSizer: byId('sim-sizer'),
      simStage: byId('sim-stage'),
      netStatus: byId('net-status'),
      onlineCount: byId('online-count'),
      railOnline: byId('rail-online'),
      selfChip: byId('self-chip'),
      selfAvatar: byId('self-avatar'),
      selfName: byId('self-name'),
      selfId: byId('self-id'),
      searchInput: byId('search-input'),
      searchResults: byId('search-results'),
      conversationList: byId('conversation-list'),
      newChatBtn: byId('new-chat-btn'),
      backBtn: byId('back-btn'),
      chatAvatar: byId('chat-avatar'),
      chatTitle: byId('chat-title'),
      chatSubtitle: byId('chat-subtitle'),
      chatSearchBtn: byId('chat-search-btn'),
      messages: byId('messages'),
      composer: byId('composer'),
      input: byId('message-input'),
      sendBtn: byId('send-btn'),
      attachBtn: byId('attach-btn'),
      voiceBtn: byId('voice-btn'),
      fileInput: byId('file-input'),
      voiceFileInput: byId('voice-file-input'),
      dragHint: byId('drag-hint'),
      inspector: byId('inspector'),
      inspectorBody: byId('inspector-body'),
      inspectorBtn: byId('inspector-btn'),
      inspectorClose: byId('inspector-close'),
      scrim: byId('drawer-scrim'),
      momentsFeed: byId('moments-feed'),
      newMomentBtn: byId('new-moment-btn'),
      peerList: byId('peer-list'),
      radarSub: byId('radar-sub'),
      meAvatar: byId('me-avatar'),
      meName: byId('me-name'),
      meBio: byId('me-bio'),
      saveProfileBtn: byId('save-profile-btn'),
      mePeerId: byId('me-peer-id'),
      meP2pPort: byId('me-p2p-port'),
      meUiPort: byId('me-ui-port'),
      meDataDir: byId('me-data-dir'),
      statPeers: byId('stat-peers'),
      statBlobs: byId('stat-blobs'),
      statBytes: byId('stat-bytes'),
      statMoments: byId('stat-moments'),
      modalRoot: byId('modal-root'),
      toastHost: byId('toast-host'),
      recordingOverlay: byId('recording-overlay'),
      recordingCard: byId('recording-card'),
      recordingTimer: byId('recording-timer'),
      recordingWaves: byId('recording-waves'),
      recordingHint: byId('recording-hint'),
      unreadBadge: byId('unread-badge'),
      bottomNav: byId('bottom-nav'),
    });
  }

  // ------------------------------------------------------------------
  // Networking
  // ------------------------------------------------------------------
  async function api(path, options = {}) {
    const opts = { ...options };
    opts.headers = { 'Content-Type': 'application/json', ...(options.headers || {}) };
    const res = await fetch(path, opts);
    const text = await res.text();
    let data = {};
    try { data = text ? JSON.parse(text) : {}; } catch { data = { raw: text }; }
    if (!res.ok || data.ok === false) {
      throw new Error(data.error || `HTTP ${res.status}`);
    }
    return data;
  }

  async function uploadRaw(blob, mime) {
    const res = await fetch('/api/local/blob', {
      method: 'POST',
      headers: { 'Content-Type': mime || blob.type || 'application/octet-stream' },
      body: blob,
    });
    let data = {};
    try { data = await res.json(); } catch { data = {}; }
    if (!res.ok || data.ok === false) {
      throw new Error(data.error || `媒体上传失败 (${res.status})`);
    }
    return data;
  }

  function connectWs() {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws';
    let ws;
    try {
      ws = new WebSocket(`${proto}://${location.host}/ws`);
    } catch (err) {
      setNetStatus(false);
      scheduleReconnect();
      return;
    }
    state.ws = ws;
    ws.onopen = () => {
      state.wsConnected = true;
      state.reconnectDelay = 1000;
      setNetStatus(true);
    };
    ws.onclose = () => {
      state.wsConnected = false;
      setNetStatus(false);
      scheduleReconnect();
    };
    ws.onerror = () => { /* close handler drives reconnection */ };
    ws.onmessage = (event) => {
      let payload;
      try { payload = JSON.parse(event.data); } catch { return; }
      handleWsEvent(payload);
    };
  }

  function scheduleReconnect() {
    if (state.ws && state.ws.readyState !== WebSocket.CLOSED) return;
    const delay = state.reconnectDelay;
    state.reconnectDelay = Math.min(10000, Math.round(delay * 1.7));
    setTimeout(() => {
      if (state.ws && state.ws.readyState === WebSocket.CLOSED) connectWs();
    }, delay);
  }

  function setNetStatus(online) {
    els.netStatus.classList.toggle('online', online);
    els.netStatus.classList.toggle('offline', !online);
    const label = $('.net-label', els.netStatus);
    if (label) label.textContent = online ? '实时总线已连接' : '正在重连…';
  }

  function handleWsEvent(ev) {
    switch (ev.type) {
      case 'NEW_MESSAGE': {
        const m = ev.message;
        if (!m) return;
        const mine = m.sender_id === state.identity?.peer_id;
        if (!mine && m.conversation_id !== state.currentConversation) {
          state.unread.set(m.conversation_id, (state.unread.get(m.conversation_id) || 0) + 1);
          renderUnread();
          toast(`${m.sender_name || '节点'}: ${previewFor(m)}`, 'info');
        }
        upsertMessage(m);
        break;
      }
      case 'MESSAGE_STATUS': {
        let convId = ev.conversation_id;
        let list = convId ? (state.messages.get(convId) || []) : [];
        let msg = list.find((m) => m.msg_id === ev.msg_id);
        if (!msg) {
          // Sync flushes do not carry a conversation id; scan every loaded room.
          for (const [id, messages] of state.messages.entries()) {
            const found = messages.find((m) => m.msg_id === ev.msg_id);
            if (found) { msg = found; convId = id; break; }
          }
        }
        if (msg) {
          msg.delivery_status = ev.delivery_status;
          if (convId === state.currentConversation) renderMessages();
        }
        break;
      }
      case 'PEER_ONLINE':
      case 'PEER_OFFLINE':
        scheduleRefresh();
        break;
      case 'NEW_MOMENT_EVENT': {
        const author = ev.event?.author_id;
        if (author && author !== state.identity?.peer_id && state.currentTab !== 'moments') {
          toast('星云圈有新动态', 'info');
        }
        scheduleRefresh();
        break;
      }
      case 'BLOB_READY':
        state.blobRev += 1;
        state.presentBlobs.add(ev.blob_id);
        renderMessages();
        renderMoments();
        renderInspector();
        scheduleRefresh();
        break;
      case 'IDENTITY_UPDATED':
        if (ev.identity && ev.identity.peer_id === state.identity?.peer_id) {
          state.identity = ev.identity;
          renderIdentity();
        }
        break;
      case 'HELLO':
        scheduleRefresh();
        break;
      default:
        break;
    }
  }

  // ------------------------------------------------------------------
  // Bootstrap & render pipeline
  // ------------------------------------------------------------------
  async function refreshBootstrap() {
    const data = await api('/api/local/bootstrap');
    applyBootstrap(data);
    renderAll();
  }

  function applyBootstrap(data) {
    state.identity = data.identity || state.identity;
    state.node = data.node || state.node;
    state.peers = data.peers || [];
    state.conversations = data.conversations || [];
    state.moments = data.moments || [];
    state.media = data.media || [];
    state.pendingBlobs = data.pending_blobs || [];
    state.blobStats = data.blob_stats || { count: 0, bytes: 0 };
    state.presentBlobs = new Set((data.media || []).map((m) => m.blob_id));
    if (!state.conversations.some((c) => c.conversation_id === state.currentConversation)) {
      // Keep direct conversations that were opened manually; only fall back
      // when the peer really disappeared from the known list.
      const known = state.conversations.some((c) => c.conversation_id === state.currentConversation);
      if (!known && state.currentConversation !== 'lobby') {
        const peerId = peerIdFromConversation(state.currentConversation);
        if (peerId && !state.peers.some((p) => p.peer_id === peerId)) {
          state.currentConversation = 'lobby';
        }
      }
    }
  }

  function scheduleRefresh() {
    clearTimeout(state.refreshTimer);
    state.refreshTimer = setTimeout(() => {
      refreshBootstrap().catch((err) => console.warn('refresh failed', err));
    }, 260);
  }

  function renderAll() {
    renderIdentity();
    renderTelemetry();
    renderConversations();
    renderMessages();
    renderMoments();
    renderPeers();
    renderInspector();
    renderUnread();
    updateChatHeader();
  }

  // ------------------------------------------------------------------
  // Identity / peers helpers
  // ------------------------------------------------------------------
  const peerIdFromConversation = (convId) => {
    if (!convId || convId === 'lobby') return null;
    const self = state.identity?.peer_id;
    return convId.split(':').find((part) => part !== self) || null;
  };
  const peerForId = (peerId) => state.peers.find((p) => p.peer_id === peerId) || null;
  function colorOf(peerId) {
    if (state.identity && peerId === state.identity.peer_id) {
      return safeColor(state.identity.avatar_color);
    }
    const peer = peerForId(peerId);
    return safeColor(peer ? peer.avatar_color : '#6366f1');
  }
  function nameOf(peerId, fallback) {
    if (state.identity && peerId === state.identity.peer_id) return state.identity.nickname;
    const peer = peerForId(peerId);
    return peer ? peer.nickname : (fallback || '未知节点');
  }
  function conversationInfo(convId) {
    if (!convId || convId === 'lobby') {
      return {
        id: 'lobby', name: '星云大厅', avatar: '🌐', color: 'linear-gradient(135deg,#06b6d4,#6366f1)',
        peerId: null, status: 'online', subtitle: 'Nebula Lobby · 局域网公共频道',
      };
    }
    const peerId = peerIdFromConversation(convId);
    const peer = peerForId(peerId);
    return {
      id: convId,
      name: peer ? peer.nickname : (peerId ? peerId.slice(0, 8) : '未知会话'),
      avatar: initialOf(peer ? peer.nickname : '?'),
      color: safeColor(peer ? peer.avatar_color : '#6366f1'),
      peerId,
      status: peer ? peer.status : 'offline',
      subtitle: peer ? `${peer.status === 'online' ? '在线' : '离线'} · ${peer.last_ip}:${peer.last_p2p_port}` : '节点已离线',
    };
  }
  function receiverFor(convId) {
    if (!convId || convId === 'lobby') return 'lobby';
    return peerIdFromConversation(convId) || convId;
  }
  const isPhone = () => els.app.clientWidth < 768;
  const isTablet = () => {
    const w = els.app.clientWidth;
    return w >= 768 && w < 1280;
  };

  // ------------------------------------------------------------------
  // Rendering: conversations / unread
  // ------------------------------------------------------------------
  function renderConversations() {
    if (!els.conversationList) return;
    const items = state.conversations.slice();
    items.sort((a, b) => {
      if (b.last_at !== a.last_at) return b.last_at - a.last_at;
      if (a.conversation_id === 'lobby') return -1;
      if (b.conversation_id === 'lobby') return 1;
      return String(a.conversation_id).localeCompare(String(b.conversation_id));
    });
    els.conversationList.innerHTML = items.map((conv) => {
      const info = conversationInfo(conv.conversation_id);
      const last = conv.last_message;
      const unread = state.unread.get(conv.conversation_id) || 0;
      const preview = last ? previewFor(last, conv) : (conv.conversation_id === 'lobby'
        ? '向局域网里的所有节点问好'
        : '还没有消息，发一条试试');
      const online = info.peerId ? conv.peer_status === 'online' : true;
      const time = last ? fmtWhen(last.created_at) : '';
      return `<button type="button" class="conv-item ${conv.conversation_id === state.currentConversation ? 'active' : ''}"
              data-conv="${esc(conv.conversation_id)}" role="listitem">
        <span class="avatar avatar-lg" style="background:${esc(info.color)}">${esc(info.avatar)}</span>
        <span class="conv-main">
          <span class="conv-top">
            <span class="conv-name">${esc(info.name)}
              ${conv.conversation_id === 'lobby' ? '<span class="badge" style="margin-left:6px">Lobby</span>' : ''}
            </span>
            <span class="conv-time">${esc(time)}</span>
          </span>
          <span class="conv-preview">${esc(preview)}</span>
          <span class="conv-badges">
            ${conv.conversation_id === 'lobby' ? '<span class="status-dot online"></span><span style="font-size:10px;color:var(--muted)">公共频道</span>' : `<span class="status-dot ${online ? 'online' : ''}"></span><span style="font-size:10px;color:${online ? 'var(--online)' : 'var(--faint)'}">${online ? '在线' : '离线'}</span>`}
          </span>
        </span>
        ${unread ? `<em class="unread-dot">${unread > 99 ? '99+' : unread}</em>` : ''}
      </button>`;
    }).join('');
    const onlineTotal = state.peers.filter((p) => p.status === 'online').length + (state.identity ? 1 : 0);
    els.onlineCount.textContent = `${onlineTotal} 在线`;
    if (els.railOnline) els.railOnline.textContent = String(onlineTotal);
  }

  function renderUnread() {
    const total = Array.from(state.unread.values()).reduce((a, b) => a + b, 0);
    if (!els.unreadBadge) return;
    els.unreadBadge.classList.toggle('hidden', total === 0);
    els.unreadBadge.textContent = total > 99 ? '99+' : String(total);
    renderConversations();
  }

  function previewFor(msg) {
    if (!msg) return '';
    if (msg.msg_type === 'image') return msg.content ? `[图片] ${msg.content}` : '[图片]';
    if (msg.msg_type === 'voice') return '[语音]';
    return msg.content || '';
  }

  // ------------------------------------------------------------------
  // Rendering: chat messages
  // ------------------------------------------------------------------
  function blobUrl(id) {
    return `/api/local/blob/${encodeURIComponent(id)}?r=${state.blobRev}`;
  }

  function imageFrameHtml(blobId, meta, opts = {}) {
    const m = typeof meta === 'string' ? parseMeta(meta) : (meta || {});
    const thumb = thumbSrc(m.thumb_b64);
    const w = Number(m.width) > 0 ? Number(m.width) : 4;
    const h = Number(m.height) > 0 ? Number(m.height) : 3;
    const ratio = clamp(w / h, 0.55, 2.2);
    const style = opts.square ? '' : `style="aspect-ratio:${ratio.toFixed(3)}"`;
    return `<div class="image-frame" data-action="lightbox" data-blob-id="${esc(blobId)}"
              data-src="${esc(blobUrl(blobId))}" ${style}>
      ${thumb ? `<img class="thumb" src="${esc(thumb)}" alt="图片缩略图">` : '<div class="thumb"></div>'}
      <img class="full" src="${esc(blobUrl(blobId))}" alt="图片" loading="lazy">
      <div class="img-loader">拉取原图…</div>
    </div>`;
  }

  function voiceHtml(blobId, meta) {
    const m = typeof meta === 'string' ? parseMeta(meta) : (meta || {});
    const wave = Array.isArray(m.waveform) && m.waveform.length ? m.waveform : DEFAULT_WAVE;
    const dur = Number(m.duration_ms) || 0;
    return `<div class="voice-player" data-action="voice" data-blob-id="${esc(blobId)}"
              data-duration="${dur}">
      <button type="button" class="voice-play" aria-label="播放语音">▶</button>
      <div class="wave">${wave.slice(0, 16).map((v) => `<i style="--h:${clamp(Number(v) || 6, 5, 100)}"></i>`).join('')}</div>
      <span class="voice-dur">${esc(fmtSoundDur(dur || 1200))}</span>
    </div>`;
  }

  function statusHtml(msg) {
    if (msg.delivery_status === 'delivered') {
      return '<span class="status-chip delivered" data-status>✓✓ 已送达</span>';
    }
    if (msg.delivery_status === 'queued') {
      return '<span class="status-chip queued" data-status>◷ 离线排队中</span>';
    }
    return '<span class="status-chip" data-status>✓ 已发送</span>';
  }

  function messageRowHtml(msg) {
    const mine = msg.sender_id === state.identity?.peer_id;
    const color = colorOf(msg.sender_id);
    const initial = initialOf(msg.sender_name);
    const showAuthor = !mine && msg.conversation_id === 'lobby';
    let body = '';
    if (msg.msg_type === 'image' && msg.blob_id) body += imageFrameHtml(msg.blob_id, msg.media_meta_json);
    if (msg.msg_type === 'voice' && msg.blob_id) body += voiceHtml(msg.blob_id, msg.media_meta_json);
    if (msg.content) body += `<div class="msg-text">${esc(msg.content)}</div>`;
    const copyText = msg.content || (msg.msg_type === 'image' ? '[图片]' : '[语音]');
    return `<div class="msg-row ${mine ? 'mine' : ''}" data-msg-id="${esc(msg.msg_id)}">
      ${mine ? '' : `<span class="avatar msg-avatar" style="background:${esc(color)}">${esc(initial)}</span>`}
      <div class="msg-stack">
        ${showAuthor ? `<div class="msg-author">${esc(msg.sender_name)}</div>` : ''}
        <div class="msg-bubble">
          ${body}
          <div class="msg-actions">
            ${mine ? '' : `<button type="button" data-action="quote-msg" data-name="${esc(msg.sender_name)}">引用</button>`}
            <button type="button" data-action="copy-msg" data-text="${esc(copyText)}">复制</button>
          </div>
        </div>
        <div class="msg-meta"><span>${esc(fmtTime(msg.created_at))}</span>${mine ? statusHtml(msg) : ''}</div>
      </div>
    </div>`;
  }

  function renderMessages(forceScroll = false) {
    if (!els.messages) return;
    const conv = state.currentConversation;
    const list = state.messages.get(conv) || [];
    const nearBottom = els.messages.scrollHeight - els.messages.scrollTop - els.messages.clientHeight < 180;
    const oldScroll = els.messages.scrollTop;
    if (!list.length) {
      const info = conversationInfo(conv);
      els.messages.innerHTML = `<div class="empty-state">
        <div class="empty-orb"></div>
        <h4>${conv === 'lobby' ? '信号已就绪' : `与 ${esc(info.name)} 的私密信道`}</h4>
        <p>${conv === 'lobby'
          ? '在同一个局域网内打开另一个 Nebula 节点，即可自动发现彼此。'
          : '消息会通过 P2P 直达对方；对方离线时自动进入本地 Outbox，重新上线后立即补发。'}</p>
      </div>`;
      return;
    }
    const parts = [];
    let lastDay = null;
    for (const msg of list) {
      const day = fmtDay(msg.created_at);
      if (day !== lastDay) {
        parts.push(`<div class="day-divider">${esc(day)}</div>`);
        lastDay = day;
      }
      parts.push(messageRowHtml(msg));
    }
    els.messages.innerHTML = parts.join('');
    if (state.pendingHighlight) {
      const target = $(`[data-msg-id="${cssEscape(state.pendingHighlight)}"]`, els.messages);
      if (target) {
        target.scrollIntoView({ block: 'center' });
        target.classList.add('flash');
      }
      state.pendingHighlight = null;
    } else if (forceScroll || nearBottom) {
      els.messages.scrollTop = els.messages.scrollHeight;
    } else {
      els.messages.scrollTop = oldScroll;
    }
  }

  function cssEscape(value) {
    if (window.CSS && CSS.escape) return CSS.escape(value);
    return String(value).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  }

  function upsertMessage(msg, forceScroll = false) {
    if (!msg || !msg.conversation_id) return;
    const conv = msg.conversation_id;
    let list = state.messages.get(conv);
    if (!list) {
      if (conv !== state.currentConversation) return;
      list = [];
      state.messages.set(conv, list);
    }
    const idx = list.findIndex((m) => m.msg_id === msg.msg_id);
    if (idx >= 0) list[idx] = msg;
    else {
      list.push(msg);
      list.sort((a, b) => (a.lamport_ts - b.lamport_ts) || (a.created_at - b.created_at));
    }
    let convEntry = state.conversations.find((c) => c.conversation_id === conv);
    if (!convEntry && conv !== 'lobby') {
      const peerId = peerIdFromConversation(conv);
      const peer = peerForId(peerId);
      if (peer) {
        convEntry = {
          conversation_id: conv, peer_id: peer.peer_id, peer_name: peer.nickname,
          peer_status: peer.status, last_message: msg, last_at: msg.created_at,
        };
        state.conversations.push(convEntry);
      }
    }
    if (convEntry) {
      convEntry.last_message = msg;
      convEntry.last_at = msg.created_at;
    }
    if (conv === state.currentConversation) {
      renderMessages(forceScroll || msg.sender_id === state.identity?.peer_id);
    }
    renderConversations();
    scheduleRefresh();
  }

  async function loadMessages(conv) {
    try {
      const data = await api(`/api/local/messages?conversation_id=${encodeURIComponent(conv)}&limit=500`);
      const list = data.messages || [];
      list.sort((a, b) => (a.lamport_ts - b.lamport_ts) || (a.created_at - b.created_at));
      state.messages.set(conv, list);
      if (conv === state.currentConversation) renderMessages(true);
    } catch (err) {
      toast(`加载消息失败: ${err.message}`, 'error');
    }
  }

  // ------------------------------------------------------------------
  // Chat actions
  // ------------------------------------------------------------------
  async function openConversation(convId, opts = {}) {
    if (!convId) return;
    const prev = state.currentConversation;
    if (prev && prev !== convId) {
      state.drafts.set(prev, els.input.value);
    }
    state.currentConversation = convId;
    state.unread.delete(convId);
    els.app.dataset.mobilePane = 'main';
    if (opts.highlight) state.pendingHighlight = opts.highlight;
    updateChatHeader();
    renderConversations();
    renderUnread();
    els.input.value = state.drafts.get(convId) || '';
    autoGrowInput();
    if (!state.messages.has(convId)) await loadMessages(convId);
    else renderMessages(true);
    renderInspector();
    if (isPhone()) closeInspector();
  }

  function updateChatHeader() {
    if (!els.chatTitle) return;
    const info = conversationInfo(state.currentConversation);
    els.chatTitle.textContent = info.name;
    els.chatSubtitle.textContent = info.subtitle;
    els.chatAvatar.textContent = info.avatar;
    els.chatAvatar.style.background = info.color;
  }

  async function postMessage(payload) {
    const body = {
      conversation_id: state.currentConversation,
      receiver_id: receiverFor(state.currentConversation),
      msg_type: 'text',
      content: '',
      ...payload,
    };
    const data = await api('/api/local/messages', { method: 'POST', body: JSON.stringify(body) });
    upsertMessage(data.message, true);
    return data.message;
  }

  async function sendText() {
    const text = els.input.value.trim();
    if (!text || state.sending) return;
    state.sending = true;
    els.sendBtn.disabled = true;
    try {
      await postMessage({ msg_type: 'text', content: text });
      els.input.value = '';
      state.drafts.set(state.currentConversation, '');
      autoGrowInput();
    } catch (err) {
      toast(err.message, 'error');
    } finally {
      state.sending = false;
      els.sendBtn.disabled = false;
    }
  }

  async function sendImageFile(file, caption = '') {
    if (!file.type.startsWith('image/')) {
      toast('仅支持图片文件', 'warn');
      return;
    }
    try {
      const thumb = await makeThumb(file);
      const up = await uploadRaw(file, file.type || 'image/jpeg');
      await postMessage({
        msg_type: 'image',
        content: caption,
        blob_id: up.blob_id,
        media_meta: {
          thumb_b64: thumb.thumb_b64,
          width: thumb.width,
          height: thumb.height,
          mime_type: up.mime_type,
          size_bytes: up.size_bytes,
        },
      });
      return true;
    } catch (err) {
      toast(err.message, 'error');
      return false;
    }
  }

  async function sendAudioFile(file) {
    try {
      const analysis = await analyzeAudio(file);
      const up = await uploadRaw(file, file.type || 'audio/webm');
      await postMessage({
        msg_type: 'voice',
        blob_id: up.blob_id,
        media_meta: {
          duration_ms: analysis.duration_ms,
          waveform: analysis.waveform,
          mime_type: up.mime_type,
          size_bytes: up.size_bytes,
        },
      });
      return true;
    } catch (err) {
      toast(err.message, 'error');
      return false;
    }
  }

  async function handleFiles(files) {
    const list = Array.from(files || []);
    if (!list.length) return;
    let caption = els.input.value.trim();
    let sent = false;
    for (const file of list) {
      if (file.type.startsWith('image/')) {
        const ok = await sendImageFile(file, caption);
        if (ok) { sent = true; caption = ''; }
      } else if (file.type.startsWith('audio/')) {
        const ok = await sendAudioFile(file);
        if (ok) sent = true;
      } else {
        toast(`不支持的媒体类型: ${file.type || '未知'}`, 'warn');
      }
    }
    if (sent) {
      els.input.value = '';
      state.drafts.set(state.currentConversation, '');
      autoGrowInput();
    }
  }

  // ------------------------------------------------------------------
  // Media helpers: thumbnails, recording, waveform
  // ------------------------------------------------------------------
  function makeThumb(file) {
    return new Promise((resolve, reject) => {
      const url = URL.createObjectURL(file);
      const img = new Image();
      img.onload = () => {
        try {
          const scale = Math.min(1, 64 / Math.max(img.naturalWidth || 1, img.naturalHeight || 1));
          const w = Math.max(1, Math.round((img.naturalWidth || 64) * scale));
          const h = Math.max(1, Math.round((img.naturalHeight || 64) * scale));
          const canvas = document.createElement('canvas');
          canvas.width = w;
          canvas.height = h;
          const ctx = canvas.getContext('2d');
          ctx.drawImage(img, 0, 0, w, h);
          let quality = 0.62;
          let dataUrl = canvas.toDataURL('image/jpeg', quality);
          while (dataUrl.length > 4096 && quality > 0.3) {
            quality -= 0.08;
            dataUrl = canvas.toDataURL('image/jpeg', quality);
          }
          resolve({
            thumb_b64: dataUrl,
            width: img.naturalWidth || w,
            height: img.naturalHeight || h,
          });
        } catch (err) {
          reject(err);
        } finally {
          URL.revokeObjectURL(url);
        }
      };
      img.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error('无法解析图片'));
      };
      img.src = url;
    });
  }

  function pseudoWaveform(seed) {
    return Array.from({ length: 16 }, (_, i) => {
      const x = Math.sin((Number(seed) || 1) * 0.0007 + i * 1.71);
      return Math.round(26 + Math.abs(x) * 62);
    });
  }

  async function analyzeAudio(blob) {
    const fallback = {
      waveform: pseudoWaveform(blob.size + blob.type.length * 977),
      duration_ms: 0,
    };
    try {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return fallback;
      const ctx = new AC();
      const buffer = await ctx.decodeAudioData(await blob.arrayBuffer());
      const channel = buffer.getChannelData(0);
      const bars = 16;
      const block = Math.max(1, Math.floor(channel.length / bars));
      const wave = [];
      for (let i = 0; i < bars; i += 1) {
        let peak = 0;
        for (let j = 0; j < block; j += 1) {
          const v = Math.abs(channel[i * block + j] || 0);
          if (v > peak) peak = v;
        }
        wave.push(clamp(Math.round(peak * 135), 6, 100));
      }
      const duration = Math.round(buffer.duration * 1000);
      if (ctx.close) await ctx.close();
      return { waveform: wave, duration_ms: duration };
    } catch (_) {
      return fallback;
    }
  }

  function makeSynthWav(ms = 1600) {
    const sampleRate = 8000;
    const seconds = Math.max(0.8, Math.min(4, ms / 1000));
    const total = Math.floor(sampleRate * seconds);
    const buffer = new ArrayBuffer(44 + total * 2);
    const view = new DataView(buffer);
    const writeStr = (offset, str) => {
      for (let i = 0; i < str.length; i += 1) view.setUint8(offset + i, str.charCodeAt(i));
    };
    writeStr(0, 'RIFF');
    view.setUint32(4, 36 + total * 2, true);
    writeStr(8, 'WAVE');
    writeStr(12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeStr(36, 'data');
    view.setUint32(40, total * 2, true);
    for (let i = 0; i < total; i += 1) {
      const t = i / sampleRate;
      const envelope = Math.sin((Math.PI * i) / total);
      const sample = Math.sin(2 * Math.PI * 440 * t) * 0.35 * envelope;
      view.setInt16(44 + i * 2, sample * 32767, true);
    }
    return new Blob([buffer], { type: 'audio/wav' });
  }

  function startCaptureSession() {
    const session = {
      stream: null,
      recorder: null,
      chunks: [],
      fallback: false,
      mime: 'audio/webm',
      startedAt: Date.now(),
      started: false,
    };
    session.start = async () => {
      try {
        if (!navigator.mediaDevices?.getUserMedia || !window.MediaRecorder) {
          throw new Error('media recorder unavailable');
        }
        session.stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        const candidates = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus'];
        const mime = candidates.find((m) => {
          try { return MediaRecorder.isTypeSupported && MediaRecorder.isTypeSupported(m); } catch { return false; }
        }) || '';
        session.mime = mime || 'audio/webm';
        session.recorder = new MediaRecorder(session.stream, mime ? { mimeType: mime } : undefined);
        session.recorder.ondataavailable = (ev) => {
          if (ev.data && ev.data.size) session.chunks.push(ev.data);
        };
        session.recorder.start(250);
        session.started = true;
      } catch (_) {
        session.fallback = true;
        session.mime = 'audio/wav';
      }
    };
    session.stop = () => new Promise((resolve) => {
      const durationMs = Math.max(1, Date.now() - session.startedAt);
      const finish = () => {
        if (session.stream) {
          session.stream.getTracks().forEach((t) => t.stop());
          session.stream = null;
        }
        if (session.fallback) {
          resolve({ blob: makeSynthWav(Math.max(1200, Math.min(4000, durationMs))), mime: 'audio/wav', durationMs, fallback: true });
          return;
        }
        const chunks = session.chunks.slice();
        resolve({ blob: new Blob(chunks, { type: session.mime }), mime: session.mime, durationMs, fallback: false });
      };
      if (session.recorder && session.recorder.state !== 'inactive') {
        session.recorder.onstop = finish;
        try { session.recorder.stop(); } catch { finish(); }
      } else {
        finish();
      }
    });
    session.cancel = () => {
      if (session.stream) {
        session.stream.getTracks().forEach((t) => t.stop());
        session.stream = null;
      }
      if (session.recorder && session.recorder.state !== 'inactive') {
        try { session.recorder.onstop = null; session.recorder.stop(); } catch { /* ignore */ }
      }
    };
    return session;
  }

  function showRecordingUI(session, kind) {
    els.recordingOverlay.classList.remove('hidden');
    els.recordingCard.classList.remove('cancel');
    els.recordingHint.textContent = kind === 'moment' ? '点击屏幕任意处完成录音' : '松开结束 · 上滑取消';
    els.recordingWaves.innerHTML = Array.from({ length: 14 }, (_, i) =>
      `<i style="--h:${18 + ((i * 37) % 62)};--d:${i % 8}"></i>`).join('');
    clearInterval(state.recordingTimer);
    state.recordingTimer = setInterval(() => {
      els.recordingTimer.textContent = fmtClock(Date.now() - session.startedAt);
    }, 100);
    els.recordingTimer.textContent = '00:00';
  }

  function hideRecordingUI() {
    clearInterval(state.recordingTimer);
    state.recordingTimer = null;
    els.recordingOverlay.classList.add('hidden');
  }

  async function sendVoiceClip(clip) {
    if (!clip.fallback && (!clip.blob || clip.blob.size === 0)) {
      toast('没有采集到音频，请重试', 'warn');
      return;
    }
    if (clip.fallback) toast('未获取到麦克风，已使用合成测试语音', 'warn');
    try {
      const analysis = await analyzeAudio(clip.blob);
      const up = await uploadRaw(clip.blob, clip.mime);
      await postMessage({
        msg_type: 'voice',
        blob_id: up.blob_id,
        media_meta: {
          duration_ms: analysis.duration_ms || clip.durationMs,
          waveform: analysis.waveform,
          mime_type: up.mime_type,
          size_bytes: up.size_bytes,
        },
      });
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  function beginVoiceGesture(event) {
    if (state.voiceGesture) return;
    event.preventDefault();
    const session = startCaptureSession();
    const gesture = {
      session,
      startY: event.clientY,
      pointerId: event.pointerId,
      cancelled: false,
      startPromise: session.start(),
    };
    state.voiceGesture = gesture;
    showRecordingUI(session, 'chat');
    try { els.voiceBtn.setPointerCapture(event.pointerId); } catch { /* ignore */ }
  }

  async function endVoiceGesture() {
    const gesture = state.voiceGesture;
    if (!gesture) return;
    state.voiceGesture = null;
    hideRecordingUI();
    await gesture.startPromise.catch(() => {});
    const clip = await gesture.session.stop();
    if (gesture.cancelled) {
      toast('已取消录音', 'info');
      return;
    }
    if (clip.durationMs < 400 && !clip.fallback) {
      toast('按住说话，松开发送', 'warn');
      return;
    }
    await sendVoiceClip(clip);
  }

  // ------------------------------------------------------------------
  // Moments
  // ------------------------------------------------------------------
  function momentCardHtml(view) {
    const post = view.post;
    const meta = parseMeta(post.media_meta_json);
    const mimes = meta.mimes || {};
    const thumbs = meta.thumbs || {};
    const waveforms = meta.waveforms || {};
    const blobIds = Array.isArray(view.blob_ids) ? view.blob_ids : [];
    const audios = [];
    const images = [];
    for (const id of blobIds) {
      const mime = String(mimes[id] || '');
      if (mime.startsWith('audio/') || waveforms[id]) audios.push(id);
      else images.push(id);
    }
    const media = [];
    if (images.length) {
      media.push(`<div class="moment-media n${Math.min(9, images.length)}">${images.map((id) =>
        imageFrameHtml(id, { thumb_b64: thumbs[id], width: 1, height: 1 }, { square: true })).join('')}</div>`);
    }
    if (audios.length) {
      for (const id of audios) {
        const voiceMeta = {
          waveform: waveforms[id] || meta.waveform,
          duration_ms: meta.duration_ms,
        };
        media.push(`<div class="moment-voice">${voiceHtml(id, voiceMeta)}</div>`);
      }
    }
    const comments = (view.comments || []).map((c) =>
      `<div class="comment-line"><strong>${esc(c.author_name)}</strong>：${esc(c.content)}</div>`).join('');
    const liked = view.liked_by_me ? 'liked' : '';
    const color = colorOf(post.author_id);
    return `<article class="moment-card" data-moment="${esc(view.moment_id)}">
      <header class="moment-head">
        <span class="avatar" style="background:${esc(color)}">${esc(initialOf(post.author_name))}</span>
        <div class="moment-head-meta">
          <strong>${esc(post.author_name)}</strong>
          <small>${esc(String(post.author_id).slice(0, 12))}…</small>
        </div>
        <span class="moment-time">${esc(fmtWhen(post.created_at))}</span>
      </header>
      ${post.content ? `<div class="moment-text">${esc(post.content)}</div>` : ''}
      ${media.join('')}
      <div class="moment-actions">
        <button type="button" class="like-btn ${liked}" data-action="like" data-moment="${esc(view.moment_id)}">
          ${liked ? '💖' : '🤍'} <span>${view.like_count || 0}</span>
        </button>
        <button type="button" class="comment-btn" data-action="focus-comment" data-moment="${esc(view.moment_id)}">
          💬 ${view.comment_count || 0}
        </button>
        <span style="margin-left:auto;font-size:10px;color:var(--faint)">CAS · ${esc(view.moment_id.slice(0, 10))}…</span>
      </div>
      <div class="moment-comments">
        ${comments}
        <form class="comment-input-row comment-form" data-moment="${esc(view.moment_id)}">
          <input name="comment" maxlength="500" placeholder="写评论…" autocomplete="off">
          <button type="submit">发送</button>
        </form>
      </div>
    </article>`;
  }

  function renderMoments() {
    if (!els.momentsFeed) return;
    if (!state.moments.length) {
      els.momentsFeed.innerHTML = `<div class="empty-state">
        <div class="empty-orb"></div>
        <h4>星云圈还很安静</h4>
        <p>发布第一条动态。即使作者离线，只要局域网中曾有节点见过它，就会通过 Gossip 传染给后来上线的节点。</p>
      </div>`;
      return;
    }
    els.momentsFeed.innerHTML = state.moments.map(momentCardHtml).join('');
  }

  async function likeMoment(momentId, button) {
    if (button) {
      button.classList.add('pop');
      setTimeout(() => button.classList.remove('pop'), 520);
    }
    try {
      await api('/api/local/moments/interact', {
        method: 'POST',
        body: JSON.stringify({ moment_id: momentId, event_type: 'LIKE' }),
      });
      await refreshBootstrap();
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  async function submitComment(form) {
    const momentId = form.dataset.moment;
    const input = $('input[name="comment"]', form);
    const content = (input?.value || '').trim();
    if (!content) return;
    try {
      await api('/api/local/moments/interact', {
        method: 'POST',
        body: JSON.stringify({ moment_id: momentId, event_type: 'COMMENT', content }),
      });
      input.value = '';
      await refreshBootstrap();
    } catch (err) {
      toast(err.message, 'error');
    }
  }

  // ------------------------------------------------------------------
  // Moments composer
  // ------------------------------------------------------------------
  function openMomentComposer() {
    resetMomentDraft();
    const modal = openModal(`
      <div class="modal-head">
        <h3>发布星云动态</h3>
        <button type="button" class="icon-btn" data-action="close-modal">✕</button>
      </div>
      <textarea id="moment-text" class="modal-textarea" maxlength="2000" placeholder="分享此刻的星云…"></textarea>
      <div id="moment-draft" class="picked-grid"></div>
      <div class="pick-actions">
        <button type="button" id="moment-add-images" class="dashed-btn">🖼️ 添加图片</button>
        <button type="button" id="moment-voice-btn" class="dashed-btn">🎙️ 录制语音</button>
        <button type="button" id="moment-add-audio" class="dashed-btn">🎵 音频文件</button>
      </div>
      <p class="modal-hint">最多 9 张图片 + 1 段语音；媒体存入 CAS，消息仅携带 SHA-256 与 ≤4KB 模糊缩略图。</p>
      <div class="modal-actions">
        <button type="button" class="mini-btn" data-action="close-modal">取消</button>
        <button type="button" id="moment-publish" class="primary-btn">发布到局域网</button>
      </div>
      <input type="file" id="moment-file-input" accept="image/*" multiple hidden>
      <input type="file" id="moment-audio-input" accept="audio/*" hidden>
    `);
    state.momentModal = modal;
    const fileInput = $('#moment-file-input', modal);
    const audioInput = $('#moment-audio-input', modal);
    $('#moment-add-images', modal).addEventListener('click', () => fileInput.click());
    $('#moment-add-audio', modal).addEventListener('click', () => audioInput.click());
    $('#moment-voice-btn', modal).addEventListener('click', toggleMomentVoice);
    fileInput.addEventListener('change', () => {
      for (const file of Array.from(fileInput.files || [])) addMomentImage(file);
      fileInput.value = '';
    });
    audioInput.addEventListener('change', async () => {
      const file = (audioInput.files || [])[0];
      if (file) await addMomentAudioFile(file);
      audioInput.value = '';
    });
    $('#moment-publish', modal).addEventListener('click', publishMoment);
    renderMomentDraft();
  }

  function resetMomentDraft() {
    for (const item of state.momentDraft.items) {
      if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
    }
    state.momentDraft.items = [];
    if (state.momentRecording) {
      state.momentRecording.cancel();
      state.momentRecording = null;
      hideRecordingUI();
    }
  }

  function renderMomentDraft() {
    const holder = byId('moment-draft');
    if (!holder) return;
    holder.innerHTML = state.momentDraft.items.map((item, index) => {
      if (item.kind === 'image') {
        return `<div class="picked-item">
          <img src="${esc(item.previewUrl)}" alt="待发布图片">
          <button type="button" data-action="remove-moment-item" data-index="${index}" aria-label="移除">✕</button>
        </div>`;
      }
      const secs = ((item.analysis?.duration_ms || item.durationMs || 1200) / 1000).toFixed(1);
      return `<div class="picked-item">
        <div class="audio-tag">🎙️ ${esc(secs)}s</div>
        <button type="button" data-action="remove-moment-item" data-index="${index}" aria-label="移除">✕</button>
      </div>`;
    }).join('');
  }

  function addMomentImage(file) {
    if (!file.type.startsWith('image/')) return;
    if (state.momentDraft.items.filter((i) => i.kind === 'image').length >= 9) {
      toast('最多 9 张图片', 'warn');
      return;
    }
    state.momentDraft.items.push({
      kind: 'image', file, mime: file.type || 'image/jpeg',
      size: file.size, previewUrl: URL.createObjectURL(file),
    });
    renderMomentDraft();
  }

  async function addMomentAudioFile(file) {
    if (!file.type.startsWith('audio/')) {
      toast('请选择音频文件', 'warn');
      return;
    }
    const analysis = await analyzeAudio(file);
    state.momentDraft.items.push({
      kind: 'audio', file, mime: file.type || 'audio/webm', size: file.size,
      analysis, durationMs: analysis.duration_ms,
      previewUrl: URL.createObjectURL(file),
    });
    renderMomentDraft();
  }

  async function toggleMomentVoice() {
    if (state.momentRecording) {
      await finishMomentVoice();
      return;
    }
    const btn = byId('moment-voice-btn');
    const session = startCaptureSession();
    state.momentRecording = session;
    showRecordingUI(session, 'moment');
    if (btn) {
      btn.textContent = '⏹ 停止录音';
      btn.classList.add('recording');
    }
    await session.start();
    if (session.fallback) toast('未获取到麦克风，将使用合成测试语音', 'warn');
  }

  async function finishMomentVoice() {
    const session = state.momentRecording;
    if (!session) return;
    state.momentRecording = null;
    hideRecordingUI();
    const btn = byId('moment-voice-btn');
    if (btn) {
      btn.textContent = '🎙️ 录制语音';
      btn.classList.remove('recording');
    }
    const clip = await session.stop();
    if (!clip.fallback && (!clip.blob || clip.blob.size === 0 || clip.durationMs < 400)) {
      toast('录音太短', 'warn');
      return;
    }
    const analysis = await analyzeAudio(clip.blob);
    state.momentDraft.items.push({
      kind: 'audio', file: clip.blob, mime: clip.mime, size: clip.blob.size,
      analysis, durationMs: analysis.duration_ms || clip.durationMs,
      previewUrl: URL.createObjectURL(clip.blob),
    });
    if (state.momentDraft.items.filter((i) => i.kind === 'audio').length > 1) {
      toast('每条动态建议只附一段语音', 'warn');
    }
    renderMomentDraft();
  }

  async function publishMoment() {
    const modal = state.momentModal;
    if (!modal) return;
    const textEl = byId('moment-text');
    const content = (textEl?.value || '').trim();
    const items = state.momentDraft.items.slice();
    if (!content && !items.length) {
      toast('写点什么或添加媒体', 'warn');
      return;
    }
    if (items.filter((i) => i.kind === 'image').length > 9) {
      toast('最多 9 张图片', 'warn');
      return;
    }
    const publishBtn = byId('moment-publish');
    if (publishBtn) { publishBtn.disabled = true; publishBtn.textContent = '上传中…'; }
    const blobIds = [];
    const thumbs = {};
    const waveforms = {};
    const mimes = {};
    const sizes = {};
    let durationMs = 0;
    try {
      for (const item of items) {
        const up = await uploadRaw(item.file, item.mime);
        blobIds.push(up.blob_id);
        mimes[up.blob_id] = up.mime_type;
        sizes[up.blob_id] = up.size_bytes;
        if (item.kind === 'image') {
          const thumb = await makeThumb(item.file);
          thumbs[up.blob_id] = thumb.thumb_b64;
        } else {
          const analysis = item.analysis || await analyzeAudio(item.file);
          waveforms[up.blob_id] = analysis.waveform;
          durationMs = analysis.duration_ms || item.durationMs || 0;
        }
      }
      await api('/api/local/moments/post', {
        method: 'POST',
        body: JSON.stringify({
          content,
          blob_ids: blobIds,
          media_meta: { thumbs, waveforms, sizes, duration_ms: durationMs },
          mimes,
        }),
      });
      modal.remove();
      state.momentModal = null;
      resetMomentDraft();
      toast('动态已发布，正在向全网传染…', 'success');
      await refreshBootstrap();
    } catch (err) {
      toast(err.message, 'error');
      if (publishBtn) { publishBtn.disabled = false; publishBtn.textContent = '发布到局域网'; }
    }
  }

  // ------------------------------------------------------------------
  // Peers / radar / profile / inspector
  // ------------------------------------------------------------------
  function renderPeers() {
    if (!els.peerList) return;
    const online = state.peers.filter((p) => p.status === 'online').length + (state.identity ? 1 : 0);
    if (els.radarSub) {
      els.radarSub.textContent = `${online} 个节点在线 · UDP 48600–48609 端口池 · 广播 + 回环双通道`;
    }
    const cards = [];
    if (state.identity) {
      const id = state.identity;
      cards.push(`<div class="peer-card self">
        <div class="peer-card-top">
          <span class="avatar" style="background:${esc(safeColor(id.avatar_color))}">${esc(initialOf(id.nickname))}</span>
          <div style="min-width:0">
            <div class="peer-card-name">${esc(id.nickname)} <span class="badge">本机</span></div>
            <div class="peer-card-bio">${esc(id.bio || '')}</div>
          </div>
        </div>
        <div class="peer-kv">
          <span>PeerID <code>${esc(id.peer_id)}</code></span>
          <span>P2P <code>0.0.0.0:${esc(state.node?.p2p_port ?? '?')}</code> · UI <code>127.0.0.1:${esc(state.node?.ui_port ?? '?')}</code></span>
          <span>Lamport <code>${esc(id.lamport_clock)}</code> · 状态 <code>online</code></span>
        </div>
      </div>`);
    }
    for (const peer of state.peers) {
      const onlineCls = peer.status === 'online' ? 'online' : '';
      cards.push(`<div class="peer-card">
        <div class="peer-card-top">
          <span class="avatar" style="background:${esc(safeColor(peer.avatar_color))}">${esc(initialOf(peer.nickname))}</span>
          <div style="min-width:0;flex:1">
            <div class="peer-card-name">
              <span class="status-dot ${onlineCls}"></span>${esc(peer.nickname)}
            </div>
            <div class="peer-card-bio">${esc(peer.bio || 'Exploring the Nebula LAN...')}</div>
          </div>
        </div>
        <div class="peer-kv">
          <span>PeerID <code>${esc(peer.peer_id)}</code></span>
          <span>端点 <code>${esc(peer.last_ip)}:${esc(peer.last_p2p_port)}</code></span>
          <span>动态时钟 <code>${esc(peer.moments_head_clock)}</code> · 最近心跳 <code>${esc(fmtWhen(peer.last_seen_at))}</code></span>
        </div>
        <div class="peer-card-actions">
          <button type="button" class="mini-btn" data-action="dm" data-peer-id="${esc(peer.peer_id)}">💬 私聊</button>
          <button type="button" class="mini-btn" data-action="copy" data-copy="${esc(peer.peer_id)}">复制 PeerID</button>
        </div>
      </div>`);
    }
    if (!state.peers.length) {
      cards.push(`<div class="glass-card" style="grid-column:1/-1">
        <h3 style="margin-top:0">尚未发现其他节点</h3>
        <p class="hint">在同一台机器上再启动一个节点（不同 --data-dir），或在同一 Wi-Fi 下打开另一个 Nebula，即可在 1 秒内互相发现。</p>
      </div>`);
    }
    els.peerList.innerHTML = cards.join('');
  }

  function renderIdentity() {
    const id = state.identity;
    if (!id) return;
    const color = safeColor(id.avatar_color);
    els.selfAvatar.textContent = initialOf(id.nickname);
    els.selfAvatar.style.background = color;
    els.selfName.textContent = id.nickname;
    els.selfId.textContent = String(id.peer_id).slice(0, 8) + '…';
    els.meAvatar.textContent = initialOf(id.nickname);
    els.meAvatar.style.background = color;
    if (document.activeElement !== els.meName) els.meName.value = id.nickname;
    if (document.activeElement !== els.meBio) els.meBio.value = id.bio || '';
    els.mePeerId.textContent = id.peer_id;
    els.meP2pPort.textContent = String(state.node?.p2p_port ?? '—');
    els.meUiPort.textContent = String(state.node?.ui_port ?? '—');
    els.meDataDir.textContent = state.node?.data_dir || '—';
  }

  function renderTelemetry() {
    if (!els.statPeers) return;
    const online = state.peers.filter((p) => p.status === 'online').length + (state.identity ? 1 : 0);
    els.statPeers.textContent = String(online);
    els.statBlobs.textContent = String(state.blobStats.count || 0);
    els.statBytes.textContent = fmtBytes(state.blobStats.bytes || 0);
    els.statMoments.textContent = String(state.moments.length || 0);
  }

  function renderInspector() {
    if (!els.inspectorBody) return;
    const conv = state.currentConversation;
    const info = conversationInfo(conv);
    const peer = info.peerId ? peerForId(info.peerId) : null;
    const messages = state.messages.get(conv) || [];
    const media = [];
    const seen = new Set();
    for (const msg of messages) {
      if (!msg.blob_id || seen.has(msg.blob_id)) continue;
      seen.add(msg.blob_id);
      media.push({ id: msg.blob_id, type: msg.msg_type, meta: parseMeta(msg.media_meta_json) });
    }
    const images = media.filter((m) => m.type === 'image');
    const audios = media.filter((m) => m.type === 'voice');
    const online = state.peers.filter((p) => p.status === 'online').length + (state.identity ? 1 : 0);

    const peerSection = conv === 'lobby' ? `
      <div class="inspector-section">
        <h4>当前频道</h4>
        <div class="inspector-peer">
          <span class="avatar avatar-lg" style="background:linear-gradient(135deg,#06b6d4,#6366f1)">🌐</span>
          <div><strong>星云大厅</strong><small>局域网公共频道 · ${online} 个节点在线</small></div>
        </div>
      </div>` : `
      <div class="inspector-section">
        <h4>当前节点</h4>
        <div class="inspector-peer">
          <span class="avatar avatar-lg" style="background:${esc(info.color)}">${esc(info.avatar)}</span>
          <div><strong>${esc(info.name)}</strong><small>${peer ? (peer.status === 'online' ? '🟢 在线' : '⚪ 离线') : '未知'}</small></div>
        </div>
        <div class="inspector-kv">
          <div><span>PeerID</span><code>${esc(info.peerId || '—')}</code></div>
          <div><span>端点</span><code>${peer ? `${esc(peer.last_ip)}:${esc(peer.last_p2p_port)}` : '—'}</code></div>
          <div><span>动态时钟</span><code>${peer ? esc(peer.moments_head_clock) : '—'}</code></div>
          <div><span>最近心跳</span><code>${peer ? esc(fmtWhen(peer.last_seen_at)) : '—'}</code></div>
        </div>
      </div>`;

    const imageHtml = images.slice(0, 9).map((item) =>
      imageFrameHtml(item.id, item.meta, { square: true })).join('');
    const audioHtml = audios.map((item) => `<div class="media-list">${voiceHtml(item.id, item.meta)}</div>`).join('');

    els.inspectorBody.innerHTML = `
      ${peerSection}
      <div class="inspector-section">
        <h4>节点遥测</h4>
        <div class="stat-row" style="grid-template-columns:repeat(2,1fr);margin-bottom:0">
          <div class="stat"><strong>${online}</strong><span>在线节点</span></div>
          <div class="stat"><strong>${state.blobStats.count || 0}</strong><span>CAS 媒体</span></div>
          <div class="stat"><strong>${fmtBytes(state.blobStats.bytes || 0)}</strong><span>缓存体积</span></div>
          <div class="stat"><strong>${state.pendingBlobs.length}</strong><span>待拉取 Blob</span></div>
        </div>
        <div class="inspector-kv" style="margin-top:8px">
          <div><span>P2P 端口</span><code>${esc(state.node?.p2p_port ?? '—')}</code></div>
          <div><span>协议</span><code>${esc(state.node?.protocol || 'NEBULA_V1')}</code></div>
        </div>
      </div>
      <div class="inspector-section">
        <h4>共享媒体库</h4>
        ${imageHtml ? `<div class="media-grid">${imageHtml}</div>` : '<div class="empty-mini">暂无图片</div>'}
        ${audioHtml || ''}
      </div>
      <div class="inspector-section">
        <h4>关于 Nebula</h4>
        <p class="hint">零服务器架构：UDP 发现 + HTTP P2P + SHA-256 CAS。所有消息、动态与媒体都只存在于局域网设备本地。</p>
      </div>`;
  }

  // ------------------------------------------------------------------
  // Modals / sheets / lightbox / toast
  // ------------------------------------------------------------------
  function openModal(innerHtml) {
    const backdrop = document.createElement('div');
    backdrop.className = 'modal-backdrop';
    backdrop.innerHTML = `<div class="modal-card">${innerHtml}</div>`;
    backdrop.addEventListener('click', (ev) => {
      if (ev.target === backdrop) backdrop.remove();
    });
    els.modalRoot.appendChild(backdrop);
    return backdrop;
  }

  function openLightbox(src, caption) {
    const div = document.createElement('div');
    div.className = 'lightbox';
    div.innerHTML = `
      <div class="lightbox-bar">
        <span>${esc(caption || '图片预览')}</span>
        <button type="button" class="icon-btn" data-action="close-lightbox">✕</button>
      </div>
      <div class="lightbox-body"><img src="${esc(src)}" alt="全屏图片"></div>`;
    div.addEventListener('click', (ev) => {
      if (ev.target === div || ev.target.classList.contains('lightbox-body') || ev.target.dataset.action === 'close-lightbox') {
        div.remove();
      }
    });
    els.modalRoot.appendChild(div);
  }

  function openAttachSheet() {
    if (!isPhone()) {
      els.fileInput.click();
      return;
    }
    const sheet = document.createElement('div');
    sheet.className = 'sheet-backdrop';
    sheet.innerHTML = `<div class="sheet">
      <div class="sheet-title">发送到当前会话</div>
      <button type="button" class="sheet-item" data-action="pick-images">🖼️ 图片</button>
      <button type="button" class="sheet-item" data-action="pick-audio">🎵 音频文件</button>
      <button type="button" class="sheet-item" data-action="pick-voice-file">🎙️ 选择语音文件</button>
      <button type="button" class="sheet-item sheet-cancel" data-action="close-sheet">取消</button>
    </div>`;
    sheet.addEventListener('click', (ev) => {
      const target = ev.target.closest('[data-action]');
      if (!target) {
        if (ev.target === sheet) sheet.remove();
        return;
      }
      if (target.dataset.action === 'pick-images') { els.fileInput.accept = 'image/*'; els.fileInput.click(); }
      if (target.dataset.action === 'pick-audio' || target.dataset.action === 'pick-voice-file') {
        els.voiceFileInput.click();
      }
      sheet.remove();
    });
    els.modalRoot.appendChild(sheet);
  }

  function openNewChatModal() {
    const peers = state.peers;
    const body = peers.length ? peers.map((p) => `
      <button type="button" class="conv-item" data-action="dm" data-peer-id="${esc(p.peer_id)}">
        <span class="avatar avatar-lg" style="background:${esc(safeColor(p.avatar_color))}">${esc(initialOf(p.nickname))}</span>
        <span class="conv-main">
          <span class="conv-top"><span class="conv-name">${esc(p.nickname)}</span></span>
          <span class="conv-preview"><span class="status-dot ${p.status === 'online' ? 'online' : ''}"></span> ${p.status === 'online' ? '在线' : '离线'} · ${esc(p.last_ip)}:${esc(p.last_p2p_port)}</span>
        </span>
      </button>`).join('') : '<p class="hint">还没有发现其他节点。请在同一局域网启动另一个 Nebula。</p>';
    openModal(`
      <div class="modal-head">
        <h3>发起私聊</h3>
        <button type="button" class="icon-btn" data-action="close-modal">✕</button>
      </div>
      <div>${body}</div>`);
  }

  function toggleInspector(force) {
    const open = typeof force === 'boolean' ? force : !els.inspector.classList.contains('open');
    els.inspector.classList.toggle('open', open);
    els.scrim.classList.toggle('hidden', !open || !(isPhone() || isTablet()));
    if (open) renderInspector();
  }
  function closeInspector() {
    els.inspector.classList.remove('open');
    els.scrim.classList.add('hidden');
  }

  function toast(message, kind = 'info') {
    if (!message) return;
    const node = document.createElement('div');
    node.className = `toast ${kind}`;
    const icon = kind === 'success' ? '✅' : kind === 'error' ? '⚠️' : kind === 'warn' ? '🟡' : '💫';
    node.textContent = `${icon} ${message}`;
    els.toastHost.appendChild(node);
    setTimeout(() => {
      node.classList.add('out');
      setTimeout(() => node.remove(), 300);
    }, 3200);
  }

  async function copyText(text) {
    try {
      if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(text);
      else {
        const ta = document.createElement('textarea');
        ta.value = text;
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        ta.remove();
      }
      toast('已复制', 'success');
    } catch (_) {
      toast('复制失败', 'error');
    }
  }

  // ------------------------------------------------------------------
  // Audio playback
  // ------------------------------------------------------------------
  function stopAudio() {
    if (state.audio.el) {
      try { state.audio.el.pause(); } catch { /* ignore */ }
    }
    $$('.voice-player.playing').forEach((el) => el.classList.remove('playing'));
    const btns = $$('.voice-play');
    btns.forEach((b) => { b.textContent = '▶'; });
    state.audio.id = null;
  }

  async function toggleVoice(player) {
    const id = player.dataset.blobId;
    const duration = Number(player.dataset.duration) || 0;
    if (!id) return;
    if (state.audio.id === id && state.audio.el) {
      if (state.audio.el.paused) {
        try { await state.audio.el.play(); player.classList.add('playing'); $('.voice-play', player).textContent = '❚❚'; } catch { /* ignore */ }
      } else {
        state.audio.el.pause();
        player.classList.remove('playing');
        $('.voice-play', player).textContent = '▶';
      }
      return;
    }
    stopAudio();
    const audio = new Audio(blobUrl(id));
    state.audio = { el: audio, id, btn: player };
    audio.addEventListener('ended', () => {
      player.classList.remove('playing');
      const btn = $('.voice-play', player);
      if (btn) btn.textContent = '▶';
    });
    audio.addEventListener('error', () => {
      player.classList.remove('playing');
      const btn = $('.voice-play', player);
      if (btn) btn.textContent = '▶';
      toast('语音尚未下载完成，稍后会自动就绪', 'warn');
    });
    try {
      await audio.play();
      player.classList.add('playing');
      const btn = $('.voice-play', player);
      if (btn) btn.textContent = '❚❚';
      if (!duration) {
        const durEl = $('.voice-dur', player);
        if (durEl) durEl.textContent = fmtSoundDur(audio.duration * 1000);
      }
    } catch (_) {
      toast('无法播放该语音', 'error');
    }
  }

  // ------------------------------------------------------------------
  // Layout / viewport simulator
  // ------------------------------------------------------------------
  const SIM_SIZES = {
    phone: [390, 844],
    'tablet-portrait': [820, 1180],
    'tablet-landscape': [1180, 820],
    desktop: [1440, 900],
  };

  function applySim(mode) {
    state.sim = mode in SIM_SIZES ? mode : 'auto';
    els.app.dataset.viewportSim = state.sim;
    $$('[data-sim]').forEach((btn) => btn.classList.toggle('active', btn.dataset.sim === state.sim));
    const frame = els.simFrame;
    const sizer = els.simSizer;
    const on = state.sim !== 'auto';
    document.body.classList.toggle('sim-on', on);
    if (!on) {
      frame.style.position = 'absolute';
      frame.style.inset = '0';
      frame.style.left = '';
      frame.style.top = '';
      frame.style.width = '100%';
      frame.style.height = '100%';
      frame.style.transform = 'none';
      sizer.style.width = '100%';
      sizer.style.height = '100%';
      sizer.style.margin = '0';
      return;
    }
    const [w, h] = SIM_SIZES[state.sim];
    const availW = Math.max(280, window.innerWidth - 64);
    const availH = Math.max(360, window.innerHeight - 64);
    const scale = Math.min(1, availW / w, availH / h);
    frame.style.position = 'absolute';
    frame.style.inset = 'auto';
    frame.style.left = '0';
    frame.style.top = '0';
    frame.style.width = `${w}px`;
    frame.style.height = `${h}px`;
    frame.style.transform = `scale(${scale})`;
    sizer.style.width = `${Math.round(w * scale)}px`;
    sizer.style.height = `${Math.round(h * scale)}px`;
    sizer.style.margin = 'auto';
  }

  function syncViewportHeight() {
    if (window.visualViewport) {
      document.documentElement.style.setProperty('--vvh', `${Math.round(window.visualViewport.height)}px`);
    }
  }

  // ------------------------------------------------------------------
  // Tab / navigation
  // ------------------------------------------------------------------
  function setTab(tab, opts = {}) {
    state.currentTab = tab;
    els.app.dataset.tab = tab;
    $$('#rail .rail-btn').forEach((b) => b.classList.toggle('active', b.dataset.tab === tab));
    $$('#bottom-nav .bottom-btn').forEach((b) => b.classList.toggle('active', b.dataset.tab === tab));
    ['chat', 'moments', 'radar', 'me'].forEach((name) => {
      const view = byId(`view-${name}`);
      if (view) view.classList.toggle('hidden', name !== tab);
    });
    if (tab === 'chat') {
      if (isPhone()) els.app.dataset.mobilePane = opts.pane || 'list';
      else els.app.dataset.mobilePane = 'main';
    } else {
      els.app.dataset.mobilePane = 'main';
      if (tab === 'moments') refreshBootstrap().catch(() => {});
      if (tab === 'radar') renderPeers();
      if (tab === 'me') { renderIdentity(); renderTelemetry(); }
    }
    closeInspector();
    if (tab === 'chat' && !isPhone()) {
      // Desktop/tablet always keep the room visible.
      renderMessages();
    }
  }

  // ------------------------------------------------------------------
  // Event wiring
  // ------------------------------------------------------------------
  function bindEvents() {
    $$('[data-sim]').forEach((btn) => btn.addEventListener('click', () => applySim(btn.dataset.sim)));

    $$('#rail .rail-btn').forEach((btn) => btn.addEventListener('click', () => setTab(btn.dataset.tab)));
    $$('#bottom-nav .bottom-btn').forEach((btn) => btn.addEventListener('click', () => {
      if (btn.dataset.tab === 'chat') setTab('chat', { pane: 'list' });
      else setTab(btn.dataset.tab);
    }));

    els.selfChip.addEventListener('click', () => setTab('me'));
    els.backBtn.addEventListener('click', () => {
      els.app.dataset.mobilePane = 'list';
      setTab('chat', { pane: 'list' });
    });
    els.inspectorBtn.addEventListener('click', () => toggleInspector());
    els.inspectorClose.addEventListener('click', closeInspector);
    els.scrim.addEventListener('click', closeInspector);

    els.conversationList.addEventListener('click', (ev) => {
      const item = ev.target.closest('.conv-item[data-conv]');
      if (!item) return;
      openConversation(item.dataset.conv);
    });

    els.searchInput.addEventListener('input', () => {
      clearTimeout(state.searchTimer);
      const q = els.searchInput.value.trim();
      if (!q) {
        state.searchResults = null;
        els.searchResults.classList.add('hidden');
        els.conversationList.classList.remove('hidden');
        return;
      }
      state.searchTimer = setTimeout(async () => {
        try {
          const data = await api(`/api/local/messages?search=${encodeURIComponent(q)}&limit=80`);
          renderSearchResults(q, data.messages || []);
        } catch (err) {
          toast(err.message, 'error');
        }
      }, 260);
    });

    els.chatSearchBtn.addEventListener('click', () => {
      if (isPhone()) els.app.dataset.mobilePane = 'list';
      els.searchInput.focus();
    });

    els.messages.addEventListener('load', (ev) => {
      if (ev.target.matches('img.full')) {
        const frame = ev.target.closest('.image-frame');
        if (frame) frame.classList.add('ready');
      }
    }, true);
    els.messages.addEventListener('error', (ev) => {
      if (ev.target.matches('img.full')) {
        const frame = ev.target.closest('.image-frame');
        if (frame) frame.classList.remove('ready');
      }
    }, true);
    els.momentsFeed.addEventListener('load', (ev) => {
      if (ev.target.matches('img.full')) {
        const frame = ev.target.closest('.image-frame');
        if (frame) frame.classList.add('ready');
      }
    }, true);
    els.momentsFeed.addEventListener('error', (ev) => {
      if (ev.target.matches('img.full')) {
        const frame = ev.target.closest('.image-frame');
        if (frame) frame.classList.remove('ready');
      }
    }, true);
    els.inspectorBody.addEventListener('load', (ev) => {
      if (ev.target.matches('img.full')) {
        const frame = ev.target.closest('.image-frame');
        if (frame) frame.classList.add('ready');
      }
    }, true);

    els.messages.addEventListener('click', (ev) => {
      const image = ev.target.closest('[data-action="lightbox"]');
      if (image) {
        openLightbox(image.dataset.src, 'Nebula · 图片');
      }
    });

    els.input.addEventListener('input', () => {
      state.drafts.set(state.currentConversation, els.input.value);
      autoGrowInput();
    });
    els.input.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
        if (!isPhone()) {
          ev.preventDefault();
          sendText();
        }
      }
    });
    els.sendBtn.addEventListener('click', sendText);

    els.attachBtn.addEventListener('click', openAttachSheet);
    els.fileInput.addEventListener('change', async () => {
      const files = Array.from(els.fileInput.files || []);
      els.fileInput.value = '';
      els.fileInput.accept = 'image/*,audio/*';
      await handleFiles(files);
    });
    els.voiceFileInput.addEventListener('change', async () => {
      const files = Array.from(els.voiceFileInput.files || []);
      els.voiceFileInput.value = '';
      await handleFiles(files);
    });

    els.voiceBtn.addEventListener('pointerdown', beginVoiceGesture);
    window.addEventListener('pointermove', (ev) => {
      const gesture = state.voiceGesture;
      if (!gesture || ev.pointerId !== gesture.pointerId) return;
      const dy = ev.clientY - gesture.startY;
      gesture.cancelled = dy < -70;
      els.recordingCard.classList.toggle('cancel', gesture.cancelled);
      els.recordingHint.textContent = gesture.cancelled ? '松开取消' : '松开结束 · 上滑取消';
    });
    window.addEventListener('pointerup', async (ev) => {
      const gesture = state.voiceGesture;
      if (!gesture || ev.pointerId !== gesture.pointerId) return;
      await endVoiceGesture();
    });
    window.addEventListener('pointercancel', async () => {
      const gesture = state.voiceGesture;
      if (!gesture) return;
      gesture.cancelled = true;
      await endVoiceGesture();
    });

    els.recordingOverlay.addEventListener('click', async () => {
      if (state.momentRecording) {
        await finishMomentVoice();
      }
    });

    // Drag & drop — bound only on the chat view so bubbling from inner
    // elements cannot dispatch the same drop twice.
    let dragDepth = 0;
    const dragTarget = byId('view-chat');
    if (dragTarget) {
      dragTarget.addEventListener('dragenter', (ev) => {
        if (!ev.dataTransfer || !Array.from(ev.dataTransfer.types || []).includes('Files')) return;
        dragDepth += 1;
        els.dragHint.classList.remove('hidden');
      });
      dragTarget.addEventListener('dragover', (ev) => {
        if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'copy';
        ev.preventDefault();
      });
      dragTarget.addEventListener('dragleave', () => {
        dragDepth = Math.max(0, dragDepth - 1);
        if (dragDepth === 0) els.dragHint.classList.add('hidden');
      });
      dragTarget.addEventListener('drop', async (ev) => {
        ev.preventDefault();
        dragDepth = 0;
        els.dragHint.classList.add('hidden');
        if (state.currentTab !== 'chat') return;
        await handleFiles(ev.dataTransfer?.files || []);
      });
    }

    document.addEventListener('paste', async (ev) => {
      if (state.currentTab !== 'chat') return;
      const items = Array.from(ev.clipboardData?.items || []);
      const files = items
        .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
        .map((item) => item.getAsFile())
        .filter(Boolean);
      if (files.length) {
        ev.preventDefault();
        await handleFiles(files);
      }
    });

    els.newChatBtn.addEventListener('click', openNewChatModal);
    els.newMomentBtn.addEventListener('click', openMomentComposer);

    els.momentsFeed.addEventListener('click', async (ev) => {
      const like = ev.target.closest('[data-action="like"]');
      if (like) {
        await likeMoment(like.dataset.moment, like);
        return;
      }
      const focusComment = ev.target.closest('[data-action="focus-comment"]');
      if (focusComment) {
        const card = focusComment.closest('.moment-card');
        const input = card ? $('input[name="comment"]', card) : null;
        if (input) input.focus();
      }
    });
    els.momentsFeed.addEventListener('submit', async (ev) => {
      const form = ev.target.closest('.comment-form');
      if (!form) return;
      ev.preventDefault();
      await submitComment(form);
    });

    els.peerList.addEventListener('click', (ev) => {
      const dm = ev.target.closest('[data-action="dm"]');
      if (dm) {
        openDm(dm.dataset.peerId);
        return;
      }
      const copy = ev.target.closest('[data-action="copy"]');
      if (copy) copyText(copy.dataset.copy);
    });

    els.saveProfileBtn.addEventListener('click', async () => {
      const nickname = els.meName.value.trim();
      const bio = els.meBio.value.trim();
      if (!nickname) {
        toast('昵称不能为空', 'warn');
        return;
      }
      els.saveProfileBtn.disabled = true;
      try {
        const data = await api('/api/local/profile', {
          method: 'PUT',
          body: JSON.stringify({ nickname, bio }),
        });
        state.identity = data.identity;
        renderIdentity();
        toast('身份已更新并广播', 'success');
      } catch (err) {
        toast(err.message, 'error');
      } finally {
        els.saveProfileBtn.disabled = false;
      }
    });

    // Global delegated actions
    document.addEventListener('click', (ev) => {
      const target = ev.target.closest('[data-action]');
      if (!target) return;
      const action = target.dataset.action;
      switch (action) {
        case 'lightbox':
          openLightbox(target.dataset.src, 'Nebula · 图片');
          break;
        case 'copy-msg':
          copyText(target.dataset.text || '');
          break;
        case 'quote-msg':
          els.input.value = `@${target.dataset.name} ${els.input.value}`;
          state.drafts.set(state.currentConversation, els.input.value);
          els.input.focus();
          autoGrowInput();
          break;
        case 'voice':
          toggleVoice(target);
          break;
        case 'copy':
          copyText(target.dataset.copy || '');
          break;
        case 'dm':
          {
            const backdrop = target.closest('.modal-backdrop');
            if (backdrop) backdrop.remove();
            openDm(target.dataset.peerId);
          }
          break;
        case 'remove-moment-item':
          {
            const index = Number(target.dataset.index);
            const item = state.momentDraft.items[index];
            if (item?.previewUrl) URL.revokeObjectURL(item.previewUrl);
            state.momentDraft.items.splice(index, 1);
            renderMomentDraft();
          }
          break;
        case 'close-lightbox':
          target.closest('.lightbox')?.remove();
          break;
        case 'close-sheet':
          target.closest('.sheet-backdrop')?.remove();
          break;
        case 'close-modal':
          target.closest('.modal-backdrop')?.remove();
          if (target.closest('.modal-backdrop') === state.momentModal) state.momentModal = null;
          break;
        default:
          break;
      }
    });

    document.addEventListener('keydown', (ev) => {
      if (ev.key === 'Escape') {
        const lightbox = $$('.lightbox').pop();
        if (lightbox) { lightbox.remove(); return; }
        const backdrop = $$('.modal-backdrop').pop();
        if (backdrop) { backdrop.remove(); return; }
        const sheet = $$('.sheet-backdrop').pop();
        if (sheet) { sheet.remove(); return; }
        closeInspector();
      }
    });

    window.addEventListener('resize', () => {
      if (state.sim !== 'auto') applySim(state.sim);
      syncViewportHeight();
    });
    window.addEventListener('orientationchange', () => {
      setTimeout(() => {
        if (state.sim !== 'auto') applySim(state.sim);
        syncViewportHeight();
        renderMessages();
      }, 120);
    });
    if (window.visualViewport) {
      window.visualViewport.addEventListener('resize', () => {
        syncViewportHeight();
        if (state.currentTab === 'chat') {
          setTimeout(() => { els.messages.scrollTop = els.messages.scrollHeight; }, 60);
        }
      });
      window.visualViewport.addEventListener('scroll', syncViewportHeight);
    }
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) refreshBootstrap().catch(() => {});
    });
  }

  function renderSearchResults(query, messages) {
    const holder = els.searchResults;
    if (!holder) return;
    holder.classList.remove('hidden');
    els.conversationList.classList.add('hidden');
    if (!messages.length) {
      holder.innerHTML = `<div class="search-title">没有找到包含“${esc(query)}”的消息</div>`;
      return;
    }
    holder.innerHTML = `<div class="search-title">找到 ${messages.length} 条历史消息</div>` + messages.map((m) => {
      const conv = m.conversation_id === 'lobby' ? '星云大厅' : nameOf(peerIdFromConversation(m.conversation_id), m.sender_name);
      return `<button type="button" class="search-hit" data-conv="${esc(m.conversation_id)}" data-msg="${esc(m.msg_id)}">
        <strong>${esc(m.sender_name)} · ${esc(conv)}</strong>
        <span>${esc(previewFor(m))}</span>
        <em>${esc(fmtWhen(m.created_at))}</em>
      </button>`;
    }).join('');
    holder.onclick = (ev) => {
      const hit = ev.target.closest('.search-hit');
      if (!hit) return;
      els.searchInput.value = '';
      holder.classList.add('hidden');
      els.conversationList.classList.remove('hidden');
      openConversation(hit.dataset.conv, { highlight: hit.dataset.msg });
    };
  }

  function openDm(peerId) {
    if (!peerId || !state.identity) return;
    const conv = [state.identity.peer_id, peerId].sort().join(':');
    openConversation(conv);
    setTab('chat', { pane: 'main' });
  }

  function autoGrowInput() {
    if (!els.input) return;
    els.input.style.height = 'auto';
    els.input.style.height = `${Math.min(132, els.input.scrollHeight)}px`;
  }

  // ------------------------------------------------------------------
  // Boot
  // ------------------------------------------------------------------
  async function init() {
    cacheElements();
    bindEvents();
    const params = new URLSearchParams(location.search);
    const initialSim = params.get('sim');
    const initialTab = params.get('tab');
    const initialConv = params.get('conv');
    const initialPane = params.get('pane');
    applySim(initialSim || 'auto');
    syncViewportHeight();
    if (isPhone()) els.app.dataset.mobilePane = 'list';
    try {
      await refreshBootstrap();
      await loadMessages(state.currentConversation);
      updateChatHeader();
    } catch (err) {
      toast(`无法连接 Nebula 内核: ${err.message}`, 'error');
    }
    if (initialTab && ['chat', 'moments', 'radar', 'me'].includes(initialTab)) {
      setTab(initialTab, { pane: isPhone() ? 'list' : 'main' });
    }
    if (initialConv) {
      await openConversation(initialConv);
    } else if (initialPane === 'main') {
      await openConversation(state.currentConversation);
    }
    connectWs();
    autoGrowInput();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
