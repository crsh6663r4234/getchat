// Prefix with `const foundation = <figma-foundation-ids.json>;` before use_figma.
const page=await figma.getNodeByIdAsync(foundation.pages.screens);
await figma.setCurrentPageAsync(page);
if(page.children.length) throw new Error('Conversation page already has content. Update saved IDs instead of duplicating screens.');
await Promise.all(['Regular','Medium','Semi Bold','Bold'].map(style=>figma.loadFontAsync({family:'Inter',style})));
const variables=Object.fromEntries(await Promise.all(Object.entries(foundation.variables.Dark).map(async([k,id])=>[k,await figma.variables.getVariableByIdAsync(id)])));
const components=Object.fromEntries(await Promise.all(Object.entries(foundation.components).map(async([k,id])=>[k,await figma.getNodeByIdAsync(id)])));
const metrics=Object.fromEntries(await Promise.all(Object.entries(foundation.variables.metrics).map(async([k,id])=>[k,await figma.variables.getVariableByIdAsync(id)])));
const ids=[],screens={},actions={},instances=[];
const keep=n=>{ids.push(n.id);return n;};
const fill=key=>figma.variables.setBoundVariableForPaint({type:'SOLID',color:{r:0,g:0,b:0}},'color',variables[key]);
function box(name,direction='VERTICAL',color=null,gap=12,padding=0){const n=keep(figma.createAutoLayout(direction));n.name=name;n.fills=color?[fill(color)]:[];n.itemSpacing=gap;if(metrics[gap])n.setBoundVariable('itemSpacing',metrics[gap]);for(const k of ['paddingTop','paddingBottom','paddingLeft','paddingRight']){n[k]=padding;if(metrics[padding])n.setBoundVariable(k,metrics[padding]);}return n;}
function txt(value,style='Body',color='text',width){const n=keep(figma.createText());n.name=value;n.fontName={family:'Inter',style:'Regular'};n.characters=value;n.textStyleId=foundation.styles[style];n.fills=[fill(color)];if(width){n.textAutoResize='HEIGHT';n.resize(width,n.height);}return n;}
function fixed(n,w,h){n.resize(w,h);n.layoutSizingHorizontal='FIXED';n.layoutSizingVertical='FIXED';return n;}
function append(parent,node,fillWidth=false){parent.appendChild(node);if(fillWidth)node.layoutSizingHorizontal='FILL';return node;}
function spacer(parent){const n=append(parent,box('Flexible space'));n.resize(1,1);n.layoutSizingVertical='FILL';return n;}
function button(parent,label,variant='Secondary',key){const c=components['Button'+variant],n=keep(c.createInstance());n.name=label;n.setProperties({[foundation.properties['Button'+variant]]:label});parent.appendChild(n);instances.push(n.id);if(key)actions[key]=n.id;return n;}
function avatar(parent,letter,size=40){const n=keep(components.Avatar.createInstance());n.setProperties({[foundation.properties.Avatar]:letter});n.resize(size,size);parent.appendChild(n);instances.push(n.id);return n;}
function badge(parent,label){const n=keep(components.Status.createInstance());n.setProperties({[foundation.properties.Status]:label});parent.appendChild(n);instances.push(n.id);return n;}
function field(parent,label,value,width){const n=keep(components.Field.createInstance());n.setProperties({[foundation.properties.Field.label]:label,[foundation.properties.Field.value]:value});n.resize(width,n.height);parent.appendChild(n);instances.push(n.id);return n;}
function person(parent,name,summary,width,selected=false){const row=append(parent,box(name,'HORIZONTAL',selected?'surface3':null,12,12));fixed(row,width,64);row.counterAxisAlignItems='CENTER';row.cornerRadius=12;avatar(row,name[0]);const copy=append(row,box('Identity','VERTICAL',null,4));append(copy,txt(name,'Label'));append(copy,txt(summary,'Caption','muted',width-76));return row;}
function message(parent,name,content,width,own=false){const row=append(parent,box('Message / '+name,'HORIZONTAL',null,12));fixed(row,width,100);avatar(row,name[0],36);const copy=append(row,box('Message content','VERTICAL',null,4));copy.resize(width-48,90);copy.layoutSizingVertical='HUG';append(copy,txt(name+(own?' · '+(content.includes('Olur')?'Sen':'You'):'')+'   21:01','Label'));const bubble=append(copy,box('Bubble','VERTICAL',own?'surface3':'surface2',4,12));bubble.cornerRadius=12;append(bubble,txt(content,'Body','text',width-80));append(copy,txt('↩   ·   ♡','Caption','muted'));}
function makeScreen(kind,lang,width=1280,height=820,x=100,y=100,suffix=''){
 const tr=lang==='tr',T=(en,tu)=>tr?tu:en,id=kind+'-'+lang+suffix;
 const s=box(id,'HORIZONTAL','bg',0);fixed(s,width,height);s.x=x;s.y=y;s.clipsContent=true;screens[id]=s.id;
 const compact=width<1000,railWidth=compact?62:176,sideWidth=compact?238:286,mainWidth=width-railWidth-sideWidth;
 const rail=append(s,box('Main navigation','VERTICAL','surface',12,12));fixed(rail,railWidth,height);rail.counterAxisAlignItems='CENTER';
 const logo=box('GhostChat','HORIZONTAL','accent',0);fixed(logo,40,40);logo.cornerRadius=12;logo.primaryAxisAlignItems='CENTER';logo.counterAxisAlignItems='CENTER';append(logo,txt('G','Title','onAccent'));append(rail,logo);
 for(const [en,tu,icon,dest] of [['Messages','Mesajlar','◷','messages'],['Friends','Arkadaşlar','♧','friends'],['Find people','Kişi bul','⌕','people']])button(rail,compact?icon:T(en,tu),'Secondary',id+'/'+dest);
 spacer(rail);button(rail,compact?'⚙':T('Settings','Ayarlar'),'Secondary',id+'/settings');avatar(rail,'C');
 const sidebar=append(s,box('Conversations','VERTICAL','surface',12,16));fixed(sidebar,sideWidth,height);append(sidebar,txt('GHOSTCHAT','Caption','accent'));append(sidebar,txt(T('Messages','Mesajlar'),'Title'));append(sidebar,txt(T('CONVERSATIONS','SOHBETLER'),'Caption','muted'));person(sidebar,'Mira',T('See you tonight','Akşam görüşürüz'),sideWidth-32,true);person(sidebar,T('Weekend crew','Hafta sonu'),T('3 members','3 üye'),sideWidth-32);spacer(sidebar);person(sidebar,'Casper','@casper_4821',sideWidth-32);
 const main=append(s,box('Conversation','VERTICAL',null,0));fixed(main,mainWidth,height);
 const header=append(main,box('Header','HORIZONTAL','surface',12,16));fixed(header,mainWidth,80);header.counterAxisAlignItems='CENTER';avatar(header,kind==='dock'?'G':'M');const title=append(header,box('Title','VERTICAL',null,4));title.resize(mainWidth-390,44);title.layoutSizingHorizontal='FILL';append(title,txt(kind==='dock'?T('Friends','Arkadaşlar'):'Mira','Heading'));append(title,txt(kind==='dock'?T('Manage requests','İstekleri yönet'):'@mira_7192','Caption','muted'));
 if(kind==='dm')button(header,T('Call','Ara'),'Secondary',id+'/call');badge(header,T('Connected','Bağlandı'));
 const body=append(main,box('Content','VERTICAL',null,16,24));fixed(body,mainWidth,height-80);const contentWidth=mainWidth-48;
 if(kind==='dock'){
   const dock=append(body,box('Current call','VERTICAL','surface2',12,16));dock.cornerRadius=12;dock.resize(contentWidth,100);dock.layoutSizingVertical='HUG';append(dock,txt(T('Mira · In voice','Mira · Sesli görüşmede'),'Heading','good'));
   const controls=append(dock,box('Dock controls','HORIZONTAL',null,8));button(controls,T('Return to call','Aramaya dön'),'Secondary',id+'/return');button(controls,T('Mute mic','Mikrofonu kapat'),'Secondary',id+'/mute');button(controls,T('End call','Aramayı bitir'),'Danger',id+'/end');
   append(body,txt(T('Friends','Arkadaşlar'),'Title'));append(body,txt(T('Requests · 1','İstekler · 1'),'Label','accent'));person(body,'Deniz','@deniz_3408',contentWidth);const decisions=append(body,box('Friend request actions','HORIZONTAL',null,8));button(decisions,T('Accept','Kabul et'),'Primary',id+'/acceptFriend');button(decisions,T('Ignore','Yoksay'));spacer(body);return s;
 }
 const isCall=kind!=='dm';
 if(isCall){
  const stage=append(body,box('Voice call','VERTICAL','surface',16,20));stage.cornerRadius=18;stage.strokes=[fill('line')];stage.resize(contentWidth,100);stage.layoutSizingVertical='HUG';
  const callHeader=append(stage,box('Call header','HORIZONTAL',null,12));append(callHeader,txt(kind==='incoming'?T('Incoming call','Gelen arama'):T('Who’s here','Görüşmedekiler'),'Heading'));badge(callHeader,kind==='incoming'?T('Mira is calling','Mira arıyor'):T('In voice','Sesli görüşmede'));
  const members=append(stage,box('Participants','HORIZONTAL',null,12));const memberWidth=(contentWidth-52)/2;
  person(members,'Mira',T('Speaking','Konuşuyor'),memberWidth,true);person(members,'Casper',kind==='muted'?T('No microphone','Mikrofon yok'):T('You · Microphone on','Sen · Mikrofon açık'),memberWidth);
  if(kind==='share'){const shared=append(stage,box('Shared screen preview — illustrative','VERTICAL','bg',12,16));shared.resize(contentWidth-40,140);shared.cornerRadius=12;append(shared,txt(T('YOU ARE SHARING','EKRANINI PAYLAŞIYORSUN'),'Label','good'));append(shared,txt(T('A shared space for your ideas','Fikirlerin için ortak bir alan'),'Title'));append(shared,txt(T('Screen preview · refresh or stop at any time','Ekran önizlemesi · istediğin an yenile veya durdur'),'Caption','muted'));}
  if(!compact && kind!=='share' && kind!=='incoming'){const devices=append(stage,box('Devices','HORIZONTAL',null,12));field(devices,T('Microphone','Mikrofon'),T('System default','Sistem varsayılanı'),(contentWidth-52)/2);field(devices,T('Speaker','Hoparlör'),T('System default','Sistem varsayılanı'),(contentWidth-52)/2);}
  const controls=append(stage,box('Call controls','HORIZONTAL',null,8));
  if(kind==='incoming'){button(controls,T('Join call','Aramaya katıl'),'Primary',id+'/join');button(controls,T('Decline','Reddet'),'Danger',id+'/end');}
  else{button(controls,kind==='muted'?T('Try microphone','Mikrofonu dene'):T('Mute mic','Mikrofonu kapat'),'Secondary',id+'/mute');button(controls,kind==='share'?T('Stop sharing','Paylaşımı durdur'):T('Share screen','Ekranı paylaş'),'Secondary',id+'/share');button(controls,T('End call','Aramayı bitir'),'Danger',id+'/end');}
  append(stage,txt(kind==='muted'?T('No microphone found. You joined muted and can still listen.','Mikrofon bulunamadı. Sessiz katıldın; görüşmeyi dinleyebilirsin.'):kind==='incoming'?T('Join when you are ready. You can listen without a microphone.','Hazır olduğunda katıl. Mikrofon olmadan da dinleyebilirsin.'):T('Your microphone is on.','Mikrofonun açık.'),'Caption','muted',contentWidth-40));
 }
 if(!compact || !isCall){message(body,'Mira',T('Shall we look at the new design together?','Yeni tasarımı birlikte inceleyelim mi?'),contentWidth);if(!isCall)message(body,'Casper',T('Sure, let’s jump into a call.','Olur, aramaya geçelim.'),contentWidth,true);}
 spacer(body);
 const composer=append(body,box('Message composer','HORIZONTAL','surface2',12,16));fixed(composer,contentWidth,72);composer.counterAxisAlignItems='CENTER';composer.cornerRadius=14;button(composer,'+');const prompt=append(composer,txt(T('Write a message…','Mesaj yaz…'),'Body','muted'));prompt.layoutSizingHorizontal='FILL';button(composer,'↑','Primary',id+'/send');return s;
}
const kinds=['dm','incoming','call','muted','share','dock'];
for(const [li,lang] of ['en','tr'].entries())for(const [i,kind] of kinds.entries())makeScreen(kind,lang,1280,820,100+i*1420,100+li*1000);
makeScreen('call','en',860,560,100,2100,'-compact');makeScreen('muted','tr',860,560,1100,2100,'-compact');
makeScreen('dm','en',1600,1000,2100,2100,'-wide');makeScreen('call','tr',1600,1000,3900,2100,'-wide');
const links=[];
async function link(action,target){if(action.split('/')[0]===target||!actions[action]||!screens[target])return;const n=await figma.getNodeByIdAsync(actions[action]);await n.setReactionsAsync([{trigger:{type:'ON_CLICK'},actions:[{type:'NODE',destinationId:screens[target],navigation:'NAVIGATE',transition:null}]}]);links.push({from:actions[action],to:screens[target]});}
for(const lang of ['en','tr']){
 await link('dm-'+lang+'/call','call-'+lang);
 for(const kind of kinds){await link(kind+'-'+lang+'/messages','dm-'+lang);await link(kind+'-'+lang+'/friends','dock-'+lang);await link(kind+'-'+lang+'/end','dm-'+lang);}
 await link('incoming-'+lang+'/join','muted-'+lang);await link('call-'+lang+'/mute','muted-'+lang);await link('muted-'+lang+'/mute','call-'+lang);await link('call-'+lang+'/share','share-'+lang);await link('muted-'+lang+'/share','share-'+lang);await link('share-'+lang+'/share','call-'+lang);await link('dock-'+lang+'/return','call-'+lang);
}
page.flowStartingPoints=[{nodeId:screens['dm-en'],name:'English · chat → call → sharing'},{nodeId:screens['dm-tr'],name:'Türkçe · sohbet → arama → paylaşım'},{nodeId:screens['incoming-tr'],name:'Gelen arama · mikrofonsuz katılma'}];
const counts={};for(const id of Object.values(screens)){const root=await figma.getNodeByIdAsync(id);for(const n of root.findAll(()=>true))counts[n.type]=(counts[n.type]||0)+1;}
return {createdNodeIds:ids,mutatedNodeIds:[page.id,...links.map(l=>l.from)],screens,actions,links,counts,instanceCount:instances.length,fontFamilies:['Inter'],imageNodes:[],screenCount:Object.keys(screens).length};
