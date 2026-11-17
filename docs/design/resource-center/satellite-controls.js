/* Satellite UI Resource Center controls · ref-1.0
   Reference implementation for isolated demos. Not a Satellite NG transport.
   API: SatUIDesignKit.init(container); events: satui:input, satui:change.
   NO mutation of canon, facts, decisions, or project state. */
(function(global){
 'use strict';
 const initiated=new WeakSet();
 const clamp=(x,a,b)=>Math.min(b,Math.max(a,x));
 const round=(x,p=2)=>Number(Number(x).toFixed(p));
 function hsv2rgb(h,s,v){h=((h%360)+360)%360;const c=v*s,x=c*(1-Math.abs(h/60%2-1)),m=v-c;let a=[0,0,0];if(h<60)a=[c,x,0];else if(h<120)a=[x,c,0];else if(h<180)a=[0,c,x];else if(h<240)a=[0,x,c];else if(h<300)a=[x,0,c];else a=[c,0,x];return a.map(t=>Math.round((t+m)*255));}
 function rgb2hsv(r,g,b){r/=255;g/=255;b/=255;const mx=Math.max(r,g,b),mn=Math.min(r,g,b),d=mx-mn;let h=0;if(d){if(mx===r)h=60*(((g-b)/d)%6);else if(mx===g)h=60*((b-r)/d+2);else h=60*((r-g)/d+4);}return {h:(h+360)%360,s:mx===0?0:d/mx,v:mx};}
 function parseHex(v){v=String(v||'').trim().replace(/^#/,'');if(!/^(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(v))return null;if(v.length<=4)v=[...v].map(x=>x+x).join('');if(v.length===6)v+='ff';const c=[0,2,4,6].map(n=>parseInt(v.slice(n,n+2),16));return {...rgb2hsv(c[0],c[1],c[2]),a:c[3]/255};}
 function asHex(st){const rgb=hsv2rgb(st.h,st.s,st.v),a=Math.round(clamp(st.a,0,1)*255);return '#'+[...rgb,a].map(n=>n.toString(16).padStart(2,'0')).join('');}
 function notify(node,type,source){node.dispatchEvent(new CustomEvent('satui:'+type,{bubbles:true,detail:{value:node.dataset.value??'',source}}));}
 function paletteSet(node,state,source='pointer',commit=false){const mode=node.dataset.mode||'smart';const st={h:((state.h%360)+360)%360,s:clamp(state.s,0,1),v:clamp(state.v,0,1),a:mode==='fast'?1:clamp(state.a,0,1)};node._satState=st;node.dataset.value=asHex(st);const rgb=hsv2rgb(st.h,st.s,st.v);node.style.setProperty('--satui-h',round(st.h));node.style.setProperty('--satui-s',(st.s*100).toFixed(2)+'%');node.style.setProperty('--satui-v',(st.v*100).toFixed(2)+'%');node.style.setProperty('--satui-a',st.a.toFixed(3));node.style.setProperty('--satui-rgb',rgb.join(','));node.title=(mode==='smart'?'SMART':'FAST')+' · '+node.dataset.value+' · wheel hue'+(mode==='smart'?' · Shift+wheel alpha':' · Shift+wheel brightness');const text=node.querySelector('[data-satui-hex]');if(text&&document.activeElement!==text)text.value=node.dataset.value;notify(node,'input',source);if(commit)notify(node,'change',source);}
 function localRatio(element,event){const b=element.getBoundingClientRect();return {x:clamp((event.clientX-b.left)/Math.max(1,b.width),0,1),y:clamp((event.clientY-b.top)/Math.max(1,b.height),0,1)};}
 function paletteAt(node,target,event,commit=false){const st={...(node._satState||parseHex(node.dataset.value)||{h:160,s:1,v:1,a:1})},xy=localRatio(target,event),mode=node.dataset.mode||'smart';if(mode==='fast'){st.h=Math.round(xy.x*24)/24*360;st.v=1-Math.round(xy.y*3)/3*.72;st.s=1;st.a=1;}else if(target.matches('[data-satui-alpha]'))st.a=xy.x;else if(target.matches('[data-satui-hue]'))st.h=xy.x*360;else {st.s=xy.x;st.v=1-xy.y;}paletteSet(node,st,'pointer',commit);}
 function numericValue(wrapper){const input=wrapper.querySelector('input');const v=Number(input?.value);return Number.isFinite(v)?v:0;}
 function numericFormat(n,step){const precision=Math.min(6,Math.max(0,String(step).split('.')[1]?.length||0,2));return String(round(n,precision)).replace(/\.0+$/,'');}
 function numberSet(w,n,source='input',commit=false){const input=w.querySelector('input');const step=Number(w.dataset.step)||1;if(!Number.isFinite(n)||!input)return;input.value=numericFormat(n,step);w.dataset.value=input.value;const lo=Number(w.dataset.softMin),hi=Number(w.dataset.softMax);if(w.classList.contains('is-soft')&&Number.isFinite(lo)&&Number.isFinite(hi)&&hi>lo){const span=hi-lo;const outsideL=Math.max(0,lo-n),outsideR=Math.max(0,n-hi);w.style.setProperty('--satui-over-left',String(1-Math.exp(-outsideL/(span*.15))));w.style.setProperty('--satui-over-right',String(1-Math.exp(-outsideR/(span*.15))));}notify(w,'input',source);if(commit)notify(w,'change',source);}
 function numberStep(w,sign,event,source,commit=true){const step=Math.max(.000001,Number(w.dataset.step)||1);const lo=Number(w.dataset.softMin),hi=Number(w.dataset.softMax),value=numericValue(w);let scale=event?.shiftKey ? 10 : event?.altKey ? .2 : 1;if(w.classList.contains('is-soft')&&Number.isFinite(lo)&&Number.isFinite(hi)&&hi>lo){const span=hi-lo,dist=Math.max(0,lo-value,value-hi);scale/=1+2*Math.log1p(dist/(span*.13));}numberSet(w,value+sign*step*scale,source,commit);}
 function init(container=document){if(initiated.has(container))return;initiated.add(container);
  container.querySelectorAll('[data-satui-palette]').forEach(node=>{node.dataset.mode=node.dataset.satuiPalette;paletteSet(node,parseHex(node.dataset.value)||{h:160,s:.8,v:1,a:1},'init');});
  container.querySelectorAll('[data-satui-number]').forEach(node=>numberSet(node,numericValue(node),'init'));
  container.addEventListener('click',e=>{
    const segment=e.target.closest('[data-satui-segment] button');if(segment&&container.contains(segment)){const group=segment.closest('[data-satui-segment]');group.querySelectorAll('button').forEach(b=>b.setAttribute('aria-pressed',String(b===segment)));group.dataset.value=segment.dataset.value||segment.textContent.trim();notify(group,'change','click');return;}
    const step=e.target.closest('[data-satui-step]');if(step&&container.contains(step)){const w=step.closest('[data-satui-number]');if(w)numberStep(w,step.dataset.satuiStep==='up'?1:-1,e,'button');return;}
    const op=e.target.closest('[data-satui-open]');if(op&&container.contains(op)){const p=op.closest('[data-satui-palette]'),pop=p?.querySelector('.satui-pop');if(pop){const was=pop.hidden;container.querySelectorAll('.satui-pop').forEach(x=>x.hidden=true);pop.hidden=!was;op.setAttribute('aria-expanded',String(was));}return;}
    if(!e.target.closest('.satui-pop, [data-satui-open]'))container.querySelectorAll('.satui-pop').forEach(x=>x.hidden=true);
  });
  container.addEventListener('wheel',e=>{
    const pal=e.target.closest('[data-satui-palette]');if(pal&&container.contains(pal)&&e.target.closest('.satui-micro')){if(e.deltaY===0)return;e.preventDefault();const st={...pal._satState},dir=e.deltaY<0?1:-1;if(e.shiftKey){if(pal.dataset.mode==='fast')st.v=clamp(st.v+dir*.075,0,1);else st.a=clamp(st.a+dir*.05,0,1);}else st.h=(st.h+dir*(e.altKey?.75:15)+360)%360;paletteSet(pal,st,'wheel',true);return;}
    const number=e.target.closest('[data-satui-number]');if(number&&container.contains(number)){if(e.deltaY===0)return;e.preventDefault();numberStep(number,e.deltaY<0?1:-1,e,'wheel');return;}
    const select=e.target.closest('select[data-satui-wheel]');if(select&&container.contains(select)){e.preventDefault();const next=[...select.options].filter(x=>!x.disabled&&!x.hidden),i=next.indexOf(select.selectedOptions[0]),index=clamp(i+(e.deltaY>0?1:-1),0,next.length-1);if(next[index]){select.value=next[index].value;select.dispatchEvent(new Event('change',{bubbles:true}));}} 
  },{passive:false});
  container.addEventListener('change',e=>{const inp=e.target;if(inp.matches('[data-satui-number] input')){const w=inp.closest('[data-satui-number]');numberSet(w,Number(inp.value),'text',true);}});
  container.addEventListener('input',e=>{if(e.target.matches('[data-satui-hex]')){const pal=e.target.closest('[data-satui-palette]'),st=parseHex(e.target.value);if(pal&&st)paletteSet(pal,st,'hex');}});
  let active=null;
  container.addEventListener('pointerdown',e=>{
    const target=e.target.closest('[data-satui-sv],[data-satui-hue],[data-satui-alpha]');if(target&&container.contains(target)){const pal=target.closest('[data-satui-palette]');if(!pal||e.button!==0)return;active={kind:'palette',node:pal,target,id:e.pointerId};paletteAt(pal,target,e);e.preventDefault();target.setPointerCapture?.(e.pointerId);return;}
    const input=e.target.closest('[data-satui-number] input');if(input&&container.contains(input)&&e.button===0){active={kind:'number',node:input.closest('[data-satui-number]'),target:input,id:e.pointerId,x:e.clientX,start:numericValue(input.closest('[data-satui-number]')),dragging:false};}
  });
  container.addEventListener('pointermove',e=>{
    if(!active||active.id!==e.pointerId)return;
    if(active.kind==='palette'){paletteAt(active.node,active.target,e);return;}
    const dx=e.clientX-active.x;if(!active.dragging&&Math.abs(dx)<4)return;
    active.dragging=true;const w=active.node,step=Number(w.dataset.step)||1;const increments=Math.trunc(dx/4);let value=active.start+increments*step;const lo=Number(w.dataset.softMin),hi=Number(w.dataset.softMax);if(w.classList.contains('is-soft')&&Number.isFinite(lo)&&Number.isFinite(hi)&&hi>lo){const span=hi-lo;if(value<lo)value=lo-(lo-value)/(1+Math.log1p((lo-value)/span));else if(value>hi)value=hi+(value-hi)/(1+Math.log1p((value-hi)/span));}numberSet(w,value,'scrub');e.preventDefault();
  });
  container.addEventListener('pointerup',e=>{if(!active||active.id!==e.pointerId)return;if(active.kind==='palette')paletteAt(active.node,active.target,e,true);else if(active.dragging){numberSet(active.node,numericValue(active.node),'scrub',true);active.target.blur?.();}active=null;});
  container.addEventListener('pointercancel',()=>{active=null;});
  container.addEventListener('keydown',e=>{if(e.key==='Escape')container.querySelectorAll('.satui-pop').forEach(x=>x.hidden=true);});
 }
 global.SatUIDesignKit={init,parseHex,hsv2rgb,rgb2hsv,version:'ref-1.0'};
 if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',()=>init(document));else init(document);
})(window);
