// Predeclared bounded coverage, not a product latency acceptance threshold.
export function retainedCompositionCases() {
  const cases=[];
  const add=(mode,count,layout,cadence,samples,timing=mode!=='0')=>cases.push({mode,count,layout,cadence,samples,timing});
  for(const count of [1,4,20])add('0',count,'serial','warm',3);
  for(const count of [1,2,4,8,16,20])add('separate',count,'serial','warm',30);
  for(const count of [1,4,20])add('separate',count,'serial','warm',30,false);
  for(const count of [1,4,20])add('separate',count,'serial','cold-paced',3);
  for(const layout of ['parallel','join'])for(const count of [4,20]) {
    add('separate',count,layout,'warm',30);add('separate',count,layout,'paced',3);
  }
  add('separate',20,'unused','warm',30);add('separate',20,'unused','cold-paced',3);
  return cases;
}
