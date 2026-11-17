// Preserve sequential attempted/admission/physical-end evidence BEFORE fallible
// operations. Ending here means run input callbacks, not retained Worker lifetime.
// A read/submission error is not busy or proof of no live process.
export async function observeWarmInput({sample,attempts,submit,read,until,ended,verify,expected,clock=()=>performance.now()}) {
  const attempt={sample,outcome:'pending',admitted:null,phase:'submission',key:null,physicalEndMs:null};
  attempts.push(attempt);
  const start=clock();
  try {
    const key=await submit(attempt);
    if(key?.admissionRejected) {
      attempt.admitted=false;attempt.outcome='admission-rejected';attempt.admissionRejected=key.admissionRejected;
      throw Error(`sequential admission rejected: ${key.admissionRejected}`);
    }
    attempt.admitted=true;attempt.key=key;attempt.phase='observation';
    const snapshot=await until(async()=>{
      const value=await read(key);
      attempt.lastObservation={elapsedMs:clock()-start,state:value.state,current:value.current,physical:value.physical};
      return ended(value)?value:null;
    });
    const resultMs=clock()-start; // Preserve the original endpoint before client verification.
    attempt.physicalEndMs=resultMs;attempt.phase='verification';
    verify(snapshot,expected);
    attempt.outcome='correct';attempt.resultMs=resultMs;attempt.phase='complete';
    return {key,snapshot,resultMs};
  } catch(error) {
    if(attempt.outcome==='pending')attempt.outcome=attempt.phase==='submission'?'submission-error':
      attempt.phase==='verification'?'verification-error':'observation-error';
    attempt.failure=String(error);
    throw error;
  }
}
export function warmInputDenominators(attempts,planned) {
  return {planned,attempted:attempts.length,unsubmitted:planned-attempts.length,
    admitted:attempts.filter(a=>a.admitted===true).length,
    admissionRejected:attempts.filter(a=>a.admitted===false).length,
    unknownAdmission:attempts.filter(a=>a.admitted===null).length,
    correct:attempts.filter(a=>a.outcome==='correct').length,
    failedOrUnknown:attempts.filter(a=>a.admitted!==false&&a.outcome!=='correct').length,
    physicalEndObserved:attempts.filter(a=>a.physicalEndMs!==null).length};
}
