#!/usr/bin/env python3
"""Reference package consistency check. No runtime/core files required."""
from pathlib import Path
import json, re, sys
root=Path(__file__).resolve().parent
cat=json.loads((root/'components.registry.json').read_text('utf8'))['catalog']
html=(root/'Satellite_Design_Resource_Center.html').read_text('utf8')
css=(root/'satellite-controls.css').read_text('utf8')
js=(root/'satellite-controls.js').read_text('utf8')
errors=[]
ids=[x['id'] for x in cat]
if len(ids)!=len(set(ids)):errors.append('duplicate registry ids')
if len(cat)!=26:errors.append(f'expected 26 entries, got {len(cat)}')
code=[x for x in cat if x['status']=='REFERENCE_CODE']
if len(code)!=11:errors.append(f'expected 11 code references, got {len(code)}')
for x in cat:
 if x['status'] not in {'REFERENCE_CODE','GALLERY_REFERENCE','DESIGN_CONCEPT'}:errors.append(f'{x["id"]}: invalid status')
 if x['lab_anchor'][1:] not in re.findall(r'id="([^"]+)"',html):errors.append(f'{x["id"]}: missing lab anchor')
 if x['status']=='REFERENCE_CODE' and not x['html_example'].strip():errors.append(f'{x["id"]}: no example')
 if x['status']!='REFERENCE_CODE' and x['html_example'].strip():errors.append(f'{x["id"]}: concept falsely marked runnable')
 if x['html_example'] and '<script' in x['html_example'].lower():errors.append(f'{x["id"]}: inline script in example')
 if x['html_example'] and 'data-satui' not in x['html_example']:errors.append(f'{x["id"]}: no sample namespace')
for selector in ['.mod-card','#classicView','#modsBoard','window.satApp','Query::Lines']:
 if selector in css or selector in js: errors.append(f'NG-specific selector or identifier leaked into standalone assets: {selector}')
if not css.lstrip().startswith('/* Satellite UI Resource Center'):errors.append('unexpected CSS file')
if 'data-satui' not in css:errors.append('CSS namespace missing')
for f in ['satellite-tokens.css','satellite-controls.css','satellite-controls.js','CLAUDE_START_HERE.md','SEMANTIC_CROSSWALK.md','adr-0020-interface-direction.md','REFERENCE_CONTROLS.html']:
 if not (root/f).is_file():errors.append(f'missing package file: {f}')
print('OK: '+str(len(cat))+' registered; '+str(len(code))+' reference-code examples; scoped assets' if not errors else '\n'.join(errors))
sys.exit(bool(errors))
