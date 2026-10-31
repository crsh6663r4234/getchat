// Execute with Figma use_figma; construction ledger is docs/design-review/figma-state.json.
// Creates local source-matched assets; no account data or raster UI images are used.
const page = await figma.getNodeByIdAsync('0:1');
await figma.setCurrentPageAsync(page);
if (page.children.some(n => n.name === 'GhostChat · Foundations')) throw new Error('Foundations already exist; use the saved IDs to update them.');
await Promise.all(['Regular', 'Medium', 'Semi Bold', 'Bold'].map(style => figma.loadFontAsync({family:'Inter',style})));
page.name = '01 · Foundations';
const ids = [];
const keep = n => { ids.push(n.id); return n; };
const rgb = hex => ({r:parseInt(hex.slice(1,3),16)/255,g:parseInt(hex.slice(3,5),16)/255,b:parseInt(hex.slice(5,7),16)/255});
const palettes = {
  Dark: {bg:'#10131a',surface:'#171b24',surface2:'#1e2430',surface3:'#282f3d',line:'#303847',text:'#f1f3f8',muted:'#a0a9ba',accent:'#b5a4ff',accentStrong:'#927df2',good:'#6dd6a5',danger:'#ff7e87',onAccent:'#171225'},
  Light:{bg:'#f3f4f8',surface:'#ffffff',surface2:'#f7f7fb',surface3:'#eeedf5',line:'#e0e1e9',text:'#222532',muted:'#687082',accent:'#755de0',accentStrong:'#5d46c6',good:'#23865c',danger:'#bd4452',onAccent:'#ffffff'}
};
const themes={}, collections={};
for(const [theme,values] of Object.entries(palettes)) {
  const c=figma.variables.createVariableCollection('GhostChat / '+theme); c.renameMode(c.defaultModeId,theme); collections[theme]=c.id; themes[theme]={};
  for(const [name,hex] of Object.entries(values)) {
    const v=figma.variables.createVariable('color/'+name,c,'COLOR');
    v.scopes=['FRAME_FILL','SHAPE_FILL','TEXT_FILL','STROKE_COLOR'];
    v.setValueForMode(c.defaultModeId,{...rgb(hex),a:1});
    v.setVariableCodeSyntax('WEB','var(--'+name.replace(/[A-Z]/g,c=>'-'+c.toLowerCase())+')');
    themes[theme][name]=v;
  }
}
const metricsCollection=figma.variables.createVariableCollection('GhostChat / Metrics'); metricsCollection.renameMode(metricsCollection.defaultModeId,'Value');
const metrics={};
for(const n of [4,8,12,16,20,24,32,40,48]) {const v=figma.variables.createVariable('space/'+n,metricsCollection,'FLOAT');v.scopes=['GAP'];v.setValueForMode(metricsCollection.defaultModeId,n);metrics[n]=v;}
const radius=figma.variables.createVariable('radius/control',metricsCollection,'FLOAT');radius.scopes=['CORNER_RADIUS'];radius.setValueForMode(metricsCollection.defaultModeId,12);
const styles={};
for(const [name,size,weight,line] of [['Display',30,'Bold',38],['Title',20,'Bold',28],['Heading',16,'Semi Bold',24],['Body',14,'Regular',21],['Label',12,'Semi Bold',18],['Caption',11,'Regular',16]]) {
  const s=figma.createTextStyle();s.name='GhostChat / '+name;s.fontName={family:'Inter',style:weight};s.fontSize=size;s.lineHeight={unit:'PIXELS',value:line};styles[name]=s.id;
}
const paint=(key,theme='Dark')=>figma.variables.setBoundVariableForPaint({type:'SOLID',color:rgb(palettes[theme][key])},'color',themes[theme][key]);
const bindRadius=n=>{for(const key of ['topLeftRadius','topRightRadius','bottomLeftRadius','bottomRightRadius'])n.setBoundVariable(key,radius);};
function text(value,style='Body',color='text',width) {
  const n=keep(figma.createText());n.name=value;n.fontName={family:'Inter',style:'Regular'};n.characters=value;n.textStyleId=styles[style];n.fills=[paint(color)];
  if(width){n.textAutoResize='HEIGHT';n.resize(width,n.height);}return n;
}
function auto(name,direction='VERTICAL',fill=null,gap=16,padding=24){
  const n=keep(figma.createAutoLayout(direction));n.name=name;n.fills=fill?[paint(fill)]:[];n.setBoundVariable('itemSpacing',metrics[gap]);for(const k of ['paddingLeft','paddingRight','paddingTop','paddingBottom'])n.setBoundVariable(k,metrics[padding]);return n;
}
const board=auto('GhostChat · Foundations','VERTICAL','bg',24,32);board.resize(1024,100);board.layoutSizingVertical='HUG';board.x=100;board.y=100;
board.appendChild(text('GhostChat 4.0','Display'));board.appendChild(text('Two people. One quiet place to talk.','Title','accent'));
board.appendChild(text('Editable source: EN + TR · desktop 860 / 1280 / 1600 · no real account data','Body','muted'));
board.appendChild(text('Color and typography','Heading'));
const swatches=auto('Dark colors','HORIZONTAL',null,12,4);board.appendChild(swatches);
for(const key of ['bg','surface','surface2','accent','good','danger']){const sw=auto(key,'VERTICAL',null,8,4);swatches.appendChild(sw);const tile=keep(figma.createRectangle());tile.resize(132,60);tile.fills=[paint(key)];tile.cornerRadius=12;sw.appendChild(tile);sw.appendChild(text(key,'Label'));}
for(const name of ['Display','Title','Heading','Body','Label','Caption'])board.appendChild(text(name+' · Rahatça konuş. Talk freely.',name));
board.appendChild(text('Interaction rules','Heading'));
board.appendChild(text('Visible keyboard focus · mute state in text and color · readable errors with a next action · controls stay reachable at minimum window height · reduced-motion transitions','Body','muted',920));
const kit=auto('GhostChat · Components','VERTICAL','surface',24,24);kit.resize(1024,100);kit.layoutSizingVertical='HUG';kit.x=1220;kit.y=100;
kit.appendChild(text('Reusable components','Title'));
const components={}, properties={};
// Local buttons preserve source color, radius, explicit focus and disabled states.
const buttons=[];
for(const variant of ['Primary','Secondary','Danger'])for(const state of ['Default','Hover','Focus','Disabled']){
  const c=keep(figma.createComponent());c.name='Variant='+variant+', State='+state;c.layoutMode='HORIZONTAL';c.primaryAxisSizingMode='AUTO';c.counterAxisSizingMode='AUTO';c.primaryAxisAlignItems='CENTER';c.counterAxisAlignItems='CENTER';
  c.setBoundVariable('paddingLeft',metrics[16]);c.setBoundVariable('paddingRight',metrics[16]);c.setBoundVariable('paddingTop',metrics[12]);c.setBoundVariable('paddingBottom',metrics[12]);bindRadius(c);
  c.fills=[paint(variant==='Primary'?(state==='Hover'?'accentStrong':'accent'):(state==='Hover'?'surface3':'surface2'))];c.strokes=[paint(state==='Focus'?'accent':variant==='Danger'?'danger':'line')];c.strokeWeight=state==='Focus'?2:1;
  const t=text('Button','Label',variant==='Primary'?'onAccent':variant==='Danger'?'danger':'text');c.appendChild(t);const key=c.addComponentProperty('Label','TEXT','Button');t.componentPropertyReferences={characters:key};if(state==='Disabled')c.opacity=.45;buttons.push(c);
}
const buttonSet=keep(figma.combineAsVariants(buttons,page));buttonSet.name='GhostChat / Button';buttonSet.x=1220;buttonSet.y=450;buttonSet.fills=[paint('surface')];
buttons.forEach((n,i)=>{n.x=24+(i%4)*220;n.y=40+Math.floor(i/4)*96;});buttonSet.resizeWithoutConstraints(920,340);components.Button=buttonSet.id;
for(const variant of ['Primary','Secondary','Danger']){const c=buttons.find(n=>n.name==='Variant='+variant+', State=Default');components['Button'+variant]=c.id;properties['Button'+variant]=Object.keys(buttonSet.componentPropertyDefinitions).find(k=>k.startsWith('Label#'));const instance=keep(c.createInstance());kit.appendChild(instance);}
const avatar=keep(figma.createComponent());avatar.name='GhostChat / Avatar';avatar.description='Initial fallback; source avatar_data replaces initials when available. Speaking uses a good-color border and text status.';avatar.layoutMode='HORIZONTAL';avatar.primaryAxisAlignItems='CENTER';avatar.counterAxisAlignItems='CENTER';avatar.resize(40,40);avatar.cornerRadius=20;avatar.fills=[paint('surface3')];avatar.strokes=[paint('line')];
const initial=text('M','Heading','accent');avatar.appendChild(initial);const initialProp=avatar.addComponentProperty('Initial','TEXT','M');initial.componentPropertyReferences={characters:initialProp};avatar.x=1220;avatar.y=850;components.Avatar=avatar.id;properties.Avatar=initialProp;
const badge=keep(figma.createComponent());badge.name='GhostChat / Status';badge.layoutMode='HORIZONTAL';badge.primaryAxisSizingMode='AUTO';badge.counterAxisSizingMode='AUTO';badge.paddingLeft=12;badge.paddingRight=12;badge.paddingTop=4;badge.paddingBottom=4;badge.cornerRadius=12;badge.fills=[paint('surface2')];const badgeText=text('Connected','Caption','good');badge.appendChild(badgeText);const statusProp=badge.addComponentProperty('Label','TEXT','Connected');badgeText.componentPropertyReferences={characters:statusProp};badge.x=1380;badge.y=850;components.Status=badge.id;properties.Status=statusProp;
const input=keep(figma.createComponent());input.name='GhostChat / Field';input.layoutMode='VERTICAL';input.primaryAxisSizingMode='AUTO';input.counterAxisSizingMode='FIXED';input.resize(340,72);input.itemSpacing=8;input.fills=[];
const label=text('Display name','Label'), value=text('Casper','Body');input.appendChild(label);const box=auto('Input','HORIZONTAL','bg',8,12);box.strokes=[paint('line')];bindRadius(box);input.appendChild(box);box.layoutSizingHorizontal='FILL';box.appendChild(value);const labelProp=input.addComponentProperty('Label','TEXT','Display name'),valueProp=input.addComponentProperty('Value','TEXT','Casper');label.componentPropertyReferences={characters:labelProp};value.componentPropertyReferences={characters:valueProp};input.x=1220;input.y=960;components.Field=input.id;properties.Field={label:labelProp,value:valueProp};
const screens=keep(figma.createPage());screens.name='02 · Conversations and calls';
const states=keep(figma.createPage());states.name='03 · Account and states';
return {createdNodeIds:ids,mutatedNodeIds:[page.id],pages:{foundations:page.id,screens:screens.id,states:states.id},collections:{...collections,metrics:metricsCollection.id},variables:{Dark:Object.fromEntries(Object.entries(themes.Dark).map(([k,v])=>[k,v.id])),Light:Object.fromEntries(Object.entries(themes.Light).map(([k,v])=>[k,v.id])),metrics:Object.fromEntries(Object.entries(metrics).map(([k,v])=>[k,v.id])),radius:radius.id},styles,components,properties,board:board.id,counts:{colorVariables:24,metricVariables:10,buttonVariants:buttons.length,textStyles:Object.keys(styles).length},fonts:['Inter']};
