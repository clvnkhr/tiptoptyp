"""Small authoritative project server for real-bridge Explorer journeys.

Only HTTP/socket/store surfaces are provided. No native IPC or editor state is
injected by the fixture. File writes require the same CSRF header as Overleaf.
"""
import json
import re
import threading
from urllib.parse import parse_qs, urlsplit

PROJECT, ROOT, DOCUMENT, ASSET = (c * 24 for c in 'abcd')
PREVIEW_DOCUMENT = 'f' * 24
SOURCE = 'Fresh server-owned storage fixture.\n'
CSRF = 'offline-fixture-csrf'
ASSET_BYTES = b'overleaf storage fixture asset\x00\xff'


class FileProject:
    def __init__(self, *, preview_source=None):
        self._lock = threading.RLock()
        self._condition = threading.Condition(self._lock)
        self._entries = {
            ROOT: dict(id=ROOT, parent=None, name='', kind='folder'),
            DOCUMENT: dict(id=DOCUMENT, parent=ROOT, name='main.txt', kind='doc', source=SOURCE),
            ASSET: dict(id=ASSET, parent=ROOT, name='fixture.bin', kind='file'),
        }
        if preview_source is not None:
            self._entries[PREVIEW_DOCUMENT] = dict(id=PREVIEW_DOCUMENT, parent=ROOT, name='preview.typ',
                                                 kind='doc', source=preview_source)
        self._next_id, self._generation = 1, 0
        self._events = []
        self._stopping = False
        self.save_count = 0
        self.operations = []

    def stop(self):
        with self._condition:
            self._stopping = True
            self._condition.notify_all()

    def source(self, identity):
        with self._lock:
            return self._entries[identity]['source']

    def entry(self, path):
        with self._lock:
            for identity, entry in self._entries.items():
                if identity != ROOT and self._path(identity) == path:
                    return dict(entry)
        return None

    def _path(self, identity):
        entry = self._entries[identity]
        if identity == ROOT:
            return ''
        prefix = self._path(entry['parent'])
        return prefix + '/' + entry['name'] if prefix else entry['name']

    def _tree(self):
        def folder(identity):
            entry = self._entries[identity]
            children = [e for e in self._entries.values() if e['parent'] == identity]
            return dict(_id=identity, name=entry['name'],
                        docs=[dict(_id=e['id'], name=e['name']) for e in children if e['kind'] == 'doc'],
                        fileRefs=[dict(_id=e['id'], name=e['name']) for e in children if e['kind'] == 'file'],
                        folders=[folder(e['id']) for e in children if e['kind'] == 'folder'])
        return dict(_id=PROJECT, name='Explorer fixture', rootDoc_id=DOCUMENT if DOCUMENT in self._entries else None,
                    rootFolder=[folder(ROOT)])

    def _emit(self, event, *arguments):
        self._generation += 1
        self._events.append(dict(generation=self._generation, event=event, arguments=arguments))
        self._events = self._events[-64:]
        self._condition.notify_all()

    @staticmethod
    def _json(handler, status, value):
        handler.reply(status, json.dumps(value).encode(), 'application/json')

    def get(self, handler):
        path = urlsplit(handler.path)
        if path.path == '/fixture.js':
            handler.reply(200, self.javascript().encode(), 'application/javascript')
            return True
        if path.path == '/project/' + PROJECT:
            handler.reply(200, ('<!doctype html><meta charset="utf-8"><meta name="ol-csrfToken" content="' + CSRF + '">'
                               '<title>Explorer fixture</title><script src="/fixture.js" defer></script>').encode(), 'text/html')
            return True
        if path.path == '/fixture/files/events':
            try:
                generation = int(parse_qs(path.query).get('generation', [''])[0])
                if not 0 <= generation <= 100000:
                    raise ValueError()
            except ValueError:
                self._json(handler, 400, dict(error='Invalid generation'))
                return True
            with self._condition:
                self._condition.wait_for(lambda: self._stopping or self._generation != generation, timeout=15)
                result = dict(generation=self._generation, events=[e for e in self._events if e['generation'] > generation])
            self._json(handler, 200, result)
            return True
        match = re.fullmatch('/project/' + PROJECT + '/doc/([a-f0-9]{24})/download', path.path)
        if match:
            with self._lock:
                entry = self._entries.get(match[1])
                source = entry['source'] if entry and entry['kind'] == 'doc' else None
            handler.reply(200 if source is not None else 404, (source or '').encode(), 'text/plain; charset=utf-8')
            return True
        if path.path == '/project/' + PROJECT + '/file/' + ASSET:
            with self._lock:
                exists = ASSET in self._entries
            handler.reply(200 if exists else 404, ASSET_BYTES if exists else b'', 'application/octet-stream')
            return True
        return False

    def mutate(self, handler):
        try:
            length = int(handler.headers.get('Content-Length', '0'))
            if not 0 <= length <= 65536:
                raise ValueError('Invalid body size')
            body = handler.rfile.read(length)
            source_match = re.fullmatch(r'/fixture/source/([a-f0-9]{24})', handler.path)
            with self._condition:
                if source_match and handler.command == 'POST':
                    entry = self._entries.get(source_match[1])
                    if not entry or entry['kind'] != 'doc':
                        self._json(handler, 404, dict(error='Unknown document'))
                        return
                    entry['source'] = body.decode('utf-8')
                    self.save_count += 1
                    self._json(handler, 200, {})
                    return
                if handler.headers.get('X-Csrf-Token') != CSRF:
                    self._json(handler, 403, dict(error='Missing CSRF token'))
                    return
                value = json.loads(body) if body else {}
                if not isinstance(value, dict):
                    raise ValueError('Expected an object')
                match = re.fullmatch('/project/' + PROJECT + r'/(doc|file|folder)(?:/([a-f0-9]{24})(?:/(rename|move))?)?', handler.path)
                if not match:
                    self._json(handler, 404, dict(error='Unknown operation'))
                    return
                kind, identity, operation = match.groups()
                name = value.get('name')
                if identity is None:
                    if kind not in ('doc', 'folder') or handler.command != 'POST':
                        raise ValueError('Unsupported creation')
                    parent = value.get('parent_folder_id')
                    self._validate_name(parent, name)
                    identity = f'{self._next_id:024x}'
                    self._next_id += 1
                    entry = dict(id=identity, name=name, parent=parent, kind=kind)
                    if kind == 'doc':
                        entry['source'] = ''
                    self._entries[identity] = entry
                    self._emit('reciveNewDoc' if kind == 'doc' else 'reciveNewFolder', parent, dict(_id=identity, name=name, **({} if kind == 'doc' else dict(docs=[], fileRefs=[], folders=[]))))
                    self.operations.append(dict(action='create', id=identity, kind=kind, name=name, parent=parent))
                    self._json(handler, 200, dict(_id=identity, name=name))
                    return
                entry = self._entries.get(identity)
                if not entry or entry['kind'] != kind or identity == ROOT:
                    self._json(handler, 404, dict(error='Unknown entity'))
                    return
                if operation == 'rename' and handler.command == 'POST':
                    self._validate_name(entry['parent'], name, identity)
                    entry['name'] = name
                    self._emit('reciveEntityRename', identity, name)
                elif operation == 'move' and handler.command == 'POST':
                    parent = value.get('folder_id')
                    self._validate_name(parent, entry['name'], identity)
                    ancestor = parent
                    while ancestor is not None:
                        if ancestor == identity:
                            raise ValueError('A folder cannot contain itself')
                        ancestor = self._entries[ancestor]['parent']
                    entry['parent'] = parent
                    self._emit('reciveEntityMove', identity, parent)
                elif operation is None and handler.command == 'DELETE':
                    children = {identity}
                    while True:
                        expanded = children | {i for i, e in self._entries.items() if e['parent'] in children}
                        if expanded == children:
                            break
                        children = expanded
                    for child in children:
                        del self._entries[child]
                    self._emit('removeEntity', identity)
                else:
                    raise ValueError('Unsupported operation')
                self.operations.append(dict(action=operation or 'delete', id=identity, kind=kind))
                self._json(handler, 200, {})
        except (ValueError, UnicodeError, TypeError, KeyError) as error:
            self._json(handler, 409, dict(error=str(error)))

    def _validate_name(self, parent, name, identity=None):
        if parent not in self._entries or self._entries[parent]['kind'] != 'folder':
            raise ValueError('Unknown parent folder')
        if not isinstance(name, str) or not name or len(name) > 128 or name in ('.', '..') or any(c in name for c in '/\\\x00'):
            raise ValueError('Invalid name')
        if any(e['parent'] == parent and e['name'] == name and e['id'] != identity for e in self._entries.values()):
            raise ValueError('Name already exists')
        if len(self._entries) >= 64 and identity is None:
            raise ValueError('Fixture entry limit exceeded')

    def javascript(self):
        with self._lock:
            tree, generation = self._tree(), self._generation
            sources = {i: e['source'] for i, e in self._entries.items() if e['kind'] == 'doc'}
        script = r'''(() => {
'use strict';
const MAIN=__MAIN__, TREE=__TREE__, sources=new Map(Object.entries(__SOURCES__));
const listeners=new Map(), values=new Map(), watchers=new Map();
let generation=__GENERATION__, version=1;
function emitServer(event,...args){ for(const fn of listeners.get(event)||[]) fn(...args); }
function install(source){return {doc:{toString:()=>source},selection:{main:{anchor:0,head:0}}};}
function notify(key){for(const fn of watchers.get(key)||[]) fn(values.get(key));}
function render(){
 document.querySelectorAll('[data-fixture-file]').forEach(node=>node.remove());
 function folder(value){
  for(const child of value.folders){add(child._id,child.name,true);folder(child);}
  for(const child of value.docs)add(child._id,child.name,false);
 }
 function add(id,name,isFolder){
  const node=document.createElement('button');node.dataset.fixtureFile='true';node.dataset.fileId=id;
  node.setAttribute('role','treeitem');if(isFolder)node.setAttribute('aria-expanded','true');node.textContent=name;
  node.onclick=()=>{if(isFolder)return;
   fetch('/project/'+TREE._id+'/doc/'+id+'/download').then(r=>{if(!r.ok)throw Error('No document');return r.text();}).then(source=>{
    sources.set(id,source);values.set('editor.open_doc_id',id);view.setState(install(source));
    notify('editor.open_doc_id');notify('editor.view');socket.emit('joinDoc',id,()=>{});
    window.dispatchEvent(new CustomEvent('doc:after-opened',{detail:{docId:id}}));
   });
  };document.body.appendChild(node);
 }
 for(const root of TREE.rootFolder)folder(root);
}
function updateTree(event,args){
 function find(folder,id){if(folder._id===id)return folder;for(const child of folder.folders){const found=find(child,id);if(found)return found;}}
 function parentOf(folder,id){for(const kind of ['docs','fileRefs','folders']){if(folder[kind].some(e=>e._id===id))return [folder,kind];}for(const child of folder.folders){const found=parentOf(child,id);if(found)return found;}}
 const roots=TREE.rootFolder;
 const locate=id=>roots.map(root=>find(root,id)).find(Boolean);
 const parent=id=>roots.map(root=>parentOf(root,id)).find(Boolean);
 if(event==='reciveNewDoc'||event==='reciveNewFolder'){
  const target=locate(args[0]),kind=event==='reciveNewDoc'?'docs':'folders';if(target&&!target[kind].some(e=>e._id===args[1]._id))target[kind].push(args[1]);
 }else if(event==='reciveEntityRename'){const at=parent(args[0]);if(at)at[0][at[1]].find(e=>e._id===args[0]).name=args[1];
 }else if(event==='removeEntity'){const at=parent(args[0]);if(at)at[0][at[1]]=at[0][at[1]].filter(e=>e._id!==args[0]);
 }else if(event==='reciveEntityMove'){const at=parent(args[0]),target=locate(args[1]);if(at&&target){const entity=at[0][at[1]].find(e=>e._id===args[0]);at[0][at[1]]=at[0][at[1]].filter(e=>e._id!==args[0]);target[at[1]].push(entity);}}
 render();
}
const socket={publicId:'file-fixture-own',on(event,fn){if(!listeners.has(event))listeners.set(event,new Set());listeners.get(event).add(fn);},removeListener(event,fn){listeners.get(event)?.delete(fn);},disconnect(){emitServer('disconnect');},emit(event,...args){const cb=args.at(-1);if(event==='joinDoc'&&typeof cb==='function')cb(null,[],0,[],null,null,{canSkipLeaveDoc:true});if(event==='clientTracking.getConnectedUsers'&&typeof cb==='function')cb(null,[]);}};
const view={state:install(sources.get(MAIN)||''),setState(state){this.state=state;},dispatch(transaction){
 if(transaction.selection){this.state.selection={main:transaction.selection};return;}
 const id=values.get('editor.open_doc_id'),{from,to,insert}=transaction.changes,old=this.state.doc.toString();
 const text=old.slice(0,from)+insert+old.slice(to);sources.set(id,text);this.state=install(text);socket.emit('applyOtUpdate',id,{});
 fetch('/fixture/source/'+id,{method:'POST',body:text}).then(r=>{if(!r.ok)throw Error('Save failed');emitServer('otUpdateApplied',{doc:id,v:version++});window.dispatchEvent(new CustomEvent('doc:saved',{detail:{id}}));}).catch(()=>emitServer('otUpdateError'));
}};
values.set('editor.view',view);values.set('editor.open_doc_id',MAIN);
window.overleaf={unstable:{store:{get(key){return values.get(key);},watch(key,fn){if(!watchers.has(key))watchers.set(key,new Set());watchers.get(key).add(fn);return()=>watchers.get(key).delete(fn);}}}};
window.io={};window.io.connect=()=>socket;window.io.connect();render();
function watch(){fetch('/fixture/files/events?generation='+generation).then(r=>r.json()).then(value=>{generation=value.generation;for(const entry of value.events){updateTree(entry.event,entry.arguments);emitServer(entry.event,...entry.arguments);}watch();}).catch(()=>{});}
watch();setTimeout(()=>{emitServer('joinProjectResponse',{project:TREE,permissionsLevel:'owner',publicId:socket.publicId});socket.emit('joinDoc',MAIN,()=>{});},0);
})();'''
        for key, value in dict(MAIN=DOCUMENT, TREE=tree, SOURCES=sources, GENERATION=generation).items():
            script = script.replace('__' + key + '__', json.dumps(value))
        return script
