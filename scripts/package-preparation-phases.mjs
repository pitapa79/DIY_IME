const record=/^package-preparation id=(\d{1,3}) phase=(host-prepare|host-initial-catalog|host-plan-and-catalog-recheck|package-verify-node|package-capture-module|metadata-describe|metadata-validate|worker-profile|worker-assets|worker-launch-and-settle|worker-cleanup) event=(start|complete|aborted) us=(\d{1,15})\r?$/;

// Child stderr can be split at arbitrary bytes. Keep at most one short
// candidate line and forward only the fixed diagnostic schema.
export function preparationPhaseReader(emit) {
  let pending='', overflow=false, left=256;
  return chunk=>{
    if(left===0)return;
    const fragments=chunk.toString('utf8').split('\n');
    for(let index=0;index<fragments.length;index++) {
      const fragment=fragments[index];
      if(!overflow && pending.length+fragment.length<=160)pending+=fragment;
      else overflow=true;
      if(index===fragments.length-1)break;
      const match=!overflow && record.exec(pending);
      pending='';
      overflow=false;
      if(!match)continue;
      const [,id,phase,event,us]=match;
      if(Number(id)>=128 || (event==='start' && Number(us)!==0))continue;
      emit({id:Number(id),phase,event,elapsedUs:Number(us)});
      if(--left===0)break;
    }
  };
}
