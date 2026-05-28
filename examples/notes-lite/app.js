(function () {
  const APP_ID = 'notes-lite';
  const KEY = APP_ID + ':notes';
  let notes = [];
  let editingId = null;

  const $ = (id) => document.getElementById(id);

  async function call(method, params) {
    if (window.AppRuntime && typeof window.AppRuntime.call === 'function') {
      return window.AppRuntime.call(method, params);
    }
    console.warn('Using local mock AppRuntime for Notes Lite');
    window.__mockStorage = window.__mockStorage || new Map();
    if (method === 'storage.get') return { value: window.__mockStorage.has(params.key) ? window.__mockStorage.get(params.key) : params.defaultValue };
    if (method === 'storage.set') { window.__mockStorage.set(params.key, params.value); return { ok: true }; }
    if (method === 'storage.remove') { window.__mockStorage.delete(params.key); return { ok: true }; }
    if (method === 'notification.toast' || method === 'app.log') return { ok: true };
    throw new Error('Unknown mock method ' + method);
  }

  function escapeText(value) { return String(value == null ? '' : value); }

  async function load() {
    const result = await call('storage.get', { key: KEY, defaultValue: [] });
    notes = Array.isArray(result.value) ? result.value : [];
    render();
    await call('app.log', { level: 'info', message: 'Notes Lite loaded', data: { count: notes.length } });
  }

  async function persist(message) {
    await call('storage.set', { key: KEY, value: notes });
    await call('notification.toast', { message, level: 'success' });
    render();
  }

  function showEditor(note) {
    editingId = note ? note.id : null;
    $('note-title').value = note ? note.title : '';
    $('note-body').value = note ? note.body : '';
    $('editor').hidden = false;
    $('note-title').focus();
  }

  function hideEditor() {
    editingId = null;
    $('editor').hidden = true;
  }

  async function saveNote() {
    const title = $('note-title').value.trim();
    const body = $('note-body').value.trim();
    if (!title && !body) return;
    if (editingId) {
      notes = notes.map((note) => note.id === editingId ? { ...note, title: title || 'Untitled', body, updatedAt: Date.now() } : note);
    } else {
      notes.unshift({ id: 'note_' + Date.now(), title: title || 'Untitled', body, updatedAt: Date.now() });
    }
    hideEditor();
    await persist('Note saved');
  }

  async function deleteNote(id) {
    notes = notes.filter((note) => note.id !== id);
    await persist('Note deleted');
  }

  async function clearAll() {
    notes = [];
    await call('storage.remove', { key: KEY });
    await call('notification.toast', { message: 'All notes cleared', level: 'info' });
    render();
  }

  function render() {
    const query = $('search').value.trim().toLowerCase();
    const filtered = notes.filter((note) => (note.title + ' ' + note.body).toLowerCase().includes(query));
    $('empty').hidden = filtered.length !== 0;
    const list = $('notes');
    list.innerHTML = '';
    for (const note of filtered) {
      const item = document.createElement('li');
      item.className = 'note';
      const title = document.createElement('div');
      title.className = 'note-title';
      title.textContent = escapeText(note.title);
      const body = document.createElement('div');
      body.className = 'note-body';
      body.textContent = escapeText(note.body || 'No body');
      const actions = document.createElement('div');
      actions.className = 'note-actions';
      const edit = document.createElement('button');
      edit.textContent = 'Edit';
      edit.addEventListener('click', () => showEditor(note));
      const del = document.createElement('button');
      del.textContent = 'Delete';
      del.className = 'danger';
      del.addEventListener('click', () => deleteNote(note.id));
      actions.append(edit, del);
      item.append(title, body, actions);
      list.appendChild(item);
    }
  }

  $('new-note').addEventListener('click', () => showEditor(null));
  $('cancel-edit').addEventListener('click', hideEditor);
  $('save-note').addEventListener('click', saveNote);
  $('clear-all').addEventListener('click', clearAll);
  $('search').addEventListener('input', render);
  load().catch((err) => { console.error(err); $('empty').textContent = 'Failed to load notes: ' + err.message; });
})();
