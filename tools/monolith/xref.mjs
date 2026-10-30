#!/usr/bin/env node
// Cross-reference index of the monolith for the conspectus cursors.
//   node xref.mjs build <monolith.html> <index.json>
//   node xref.mjs sym   <index.json> <monolith.html> <name> [--refs N]
//   node xref.mjs chunk <index.json> <start> <end>        -> markdown work list for the aux cursors
import fs from 'node:fs';
import * as acorn from 'acorn';
import * as walk from 'acorn-walk';

const [cmd, ...args] = process.argv.slice(2);

function scriptBlocks(text) {
  const out = [];
  const re = /<script\b([^>]*)>([\s\S]*?)<\/script>/g;
  let m;
  while ((m = re.exec(text))) {
    if (/\bsrc=/.test(m[1])) continue;
    const bodyStart = m.index + m[0].indexOf('>') + 1;
    const line = text.slice(0, bodyStart).split('\n').length; // 1-based line of body start
    const id = (m[1].match(/id="([^"]+)"/) || [])[1] || '';
    out.push({ id, line, code: m[2] });
  }
  return out;
}

function build(htmlPath, outPath) {
  const text = fs.readFileSync(htmlPath, 'utf8');
  const blocks = scriptBlocks(text);
  const defs = {}, assigns = {}, refs = {}, errors = [];
  const add = (map, name, rec) => { (map[name] ||= []).push(rec); };
  blocks.forEach((b, bi) => {
    let ast;
    try {
      ast = acorn.parse(b.code, { ecmaVersion: 'latest', sourceType: 'script', locations: true,
        allowReturnOutsideFunction: true, allowAwaitOutsideFunction: true, allowHashBang: true });
    } catch (e) { errors.push({ block: bi, id: b.id, line: b.line, error: String(e.message) }); return; }
    const L = n => n.loc.start.line + b.line - 1;
    const E = n => n.loc.end.line + b.line - 1;
    const container = anc => {
      for (let i = anc.length - 2; i >= 0; i--) {
        const a = anc[i];
        if ((a.type === 'FunctionDeclaration' || a.type === 'FunctionExpression') && a.id) return a.id.name;
        if (a.type === 'Property' && a.key && (a.key.name || a.key.value)) return a.key.name || a.key.value;
        if (a.type === 'VariableDeclarator' && a.id && a.id.name) return a.id.name;
      }
      return '';
    };
    const depth = anc => anc.filter(a => /Function/.test(a.type)).length;
    const declNodes = new WeakSet();
    walk.fullAncestor(ast, (node, _st, anc) => {
      switch (node.type) {
        case 'FunctionDeclaration':
          if (node.id) { declNodes.add(node.id); add(defs, node.id.name, { kind: 'function', line: L(node), end: E(node), in: container(anc), depth: depth(anc) - 1, block: bi }); }
          break;
        case 'VariableDeclarator':
          if (node.id.type === 'Identifier') {
            declNodes.add(node.id);
            const init = node.init ? node.init.type : '';
            const kind = /Function/.test(init) ? 'function' : init === 'ObjectExpression' ? 'object' : init === 'ClassExpression' ? 'class' : init === 'CallExpression' ? 'call' : 'var';
            if (depth(anc) <= 1 || kind !== 'var') add(defs, node.id.name, { kind, line: L(node), end: E(node), in: container(anc), depth: depth(anc), block: bi });
          }
          break;
        case 'Property':
          if (node.value && /Function/.test(node.value.type) && node.key) {
            const name = node.key.name || node.key.value;
            if (name) { if (node.key.type === 'Identifier') declNodes.add(node.key); add(defs, String(name), { kind: 'method', line: L(node), end: E(node), in: container(anc), depth: depth(anc), block: bi }); }
          } else if (node.key && node.key.type === 'Identifier' && !node.computed) declNodes.add(node.key);
          break;
        case 'AssignmentExpression': {
          const t = node.left;
          let name = '';
          if (t.type === 'Identifier') name = t.name;
          else if (t.type === 'MemberExpression' && !t.computed && t.property.type === 'Identifier') {
            const obj = t.object.type === 'Identifier' ? t.object.name : t.object.type === 'ThisExpression' ? 'this' : '…';
            name = obj === 'window' || obj === 'globalThis' ? t.property.name : `${obj}.${t.property.name}`;
          }
          if (name) add(assigns, name, { line: L(node), rhs: node.right.type, in: container(anc), block: bi });
          break;
        }
        case 'MemberExpression':
          if (!node.computed && node.property.type === 'Identifier') declNodes.add(node.property);
          if (!node.computed && node.object.type === 'Identifier' && (node.object.name === 'window' || node.object.name === 'globalThis'))
            add(refs, node.property.name, L(node));
          break;
        case 'Identifier':
          if (!declNodes.has(node)) add(refs, node.name, L(node));
          break;
      }
    });
  });
  // keep refs only for names that are defined or assigned somewhere; dedupe lines
  const known = new Set([...Object.keys(defs), ...Object.keys(assigns)]);
  const refsOut = {};
  for (const [k, v] of Object.entries(refs)) if (known.has(k)) refsOut[k] = [...new Set(v)].sort((a, b) => a - b);
  const index = { file: htmlPath, lines: text.split('\n').length, blocks: blocks.map((b, i) => ({ i, id: b.id, line: b.line, lines: b.code.split('\n').length })), errors, defs, assigns, refs: refsOut };
  fs.writeFileSync(outPath, JSON.stringify(index));
  console.error(`blocks=${blocks.length} parseErrors=${errors.length} defs=${Object.keys(defs).length} assigns=${Object.keys(assigns).length} refNames=${Object.keys(refsOut).length}`);
  errors.forEach(e => console.error('  parse error', e));
}

function load(p) { return JSON.parse(fs.readFileSync(p, 'utf8')); }

function sym(indexPath, htmlPath, name, n = 12) {
  const ix = load(indexPath);
  const lines = fs.readFileSync(htmlPath, 'utf8').split('\n');
  const show = l => `${l}: ${lines[l - 1].trim().slice(0, 150)}`;
  console.log(`# ${name}`);
  for (const d of ix.defs[name] || []) console.log(`def    ${d.kind} ${d.line}–${d.end} in=${d.in || '(top)'} depth=${d.depth}`);
  for (const a of ix.assigns[name] || []) console.log(`assign ${show(a.line)}  [rhs=${a.rhs} in=${a.in || '(top)'}]`);
  const r = ix.refs[name] || [];
  console.log(`refs   ${r.length} lines`);
  r.slice(0, n).forEach(l => console.log('  ' + show(l)));
  if (r.length > n) console.log(`  … +${r.length - n}`);
}

function chunk(indexPath, start, end) {
  const ix = load(indexPath);
  start = +start; end = +end;
  const inChunk = l => l >= start && l <= end;
  const out = [];
  out.push(`## Связи куска ${start}–${end}`);
  // 1) forward references: used here, every definition is below the chunk
  const fwd = [], over = [], external = [];
  for (const [name, rl] of Object.entries(ix.refs)) {
    const used = rl.filter(inChunk);
    if (!used.length) continue;
    const ds = (ix.defs[name] || []).filter(d => d.depth <= 1 && d.kind !== 'var' && d.kind !== 'call');
    const as = ix.assigns[name] || [];
    const top = ds.map(d => d.line);
    if (top.length && Math.min(...top) > end) fwd.push(`- \`${name}\` используется на ${used[0]}, объявлено ниже: ${top.slice(0, 4).join(', ')}`);
  }
  // 2) definitions here that are redefined / reassigned elsewhere
  for (const [name, ds] of Object.entries(ix.defs)) {
    const here = ds.filter(d => inChunk(d.line) && d.depth <= 1);
    if (!here.length) continue;
    const elseDefs = ds.filter(d => !inChunk(d.line) && d.depth <= 1).map(d => d.line);
    const elseAssign = (ix.assigns[name] || []).filter(a => !inChunk(a.line) && /Function|Call|Identifier/.test(a.rhs)).map(a => a.line);
    if (elseDefs.length || elseAssign.length) over.push(`- \`${name}\` (${here[0].line}) — ещё объявления: ${elseDefs.slice(0, 5).join(', ') || '—'}; переназначения: ${elseAssign.slice(0, 5).join(', ') || '—'}`);
    const r = (ix.refs[name] || []).filter(l => !inChunk(l));
    if (r.length >= 5) external.push([name, r.length]);
  }
  out.push(`\n### Вперёд: используется здесь, объявлено ниже (${fwd.length})\n`, ...fwd.slice(0, 120));
  out.push(`\n### Переопределения: объявлено здесь, переобъявлено/переназначено в другом месте (${over.length})\n`, ...over.slice(0, 120));
  external.sort((a, b) => b[1] - a[1]);
  out.push(`\n### Хабы: объявлено здесь, широко используется снаружи (${external.length})\n`, ...external.slice(0, 40).map(([n, c]) => `- \`${n}\` — ${c} строк снаружи`));
  console.log(out.join('\n'));
}

if (cmd === 'build') build(args[0], args[1]);
else if (cmd === 'sym') sym(args[0], args[1], args[2], +(args[4] || 12));
else if (cmd === 'chunk') chunk(args[0], args[1], args[2]);
else { console.error('usage: build <html> <out.json> | sym <index> <html> <name> | chunk <index> <start> <end>'); process.exit(2); }
