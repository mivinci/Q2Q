import { spawn } from 'node:child_process';
import { networkFixture } from './network-fixture.mjs';
import { nativeCoreTestBinary } from './native-rtc-fixture.mjs';
import { once } from 'node:events';
export async function p2pFixture(options={}){
  const remote=options.trackUrl;
  if(remote && (!remote.startsWith('https://') || process.env.TEST_PUBLIC_P2P!=='1'))throw Error('Public fixture requires explicit TEST_PUBLIC_P2P=1 and HTTPS');
  const services=remote?{trackUrl:remote,async close(){}}:await networkFixture();let child;let stopped=false;let output='';
  try{
    child=spawn(await nativeCoreTestBinary(),['browser_publisher_bridge','--ignored','--nocapture'],{env:{...process.env,Q2Q_BROWSER_TRACK:services.trackUrl,Q2Q_BROWSER_RATE:'131072',...(remote?{Q2Q_BROWSER_ALLOW_REMOTE:'1'}:{})},stdio:['pipe','pipe','pipe']});
    const finished=new Promise((resolve,reject)=>{child.once('exit',code=>code===0?resolve():reject(Error(`native publisher failed ${code}: ${output}`)));child.once('error',reject);});void finished.catch(()=>{});
    const seed=await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error(`native publisher startup timeout: ${output}`)),20000);
      child.stdout.on('data',bytes=>{output=(output+bytes).slice(-16384);const match=/Q2Q_BROWSER_SEED ([A-Za-z0-9+/=]+)/.exec(output);const external=/Q2Q_BROWSER_EXTERNAL_SEED ([A-Za-z0-9+/=]+)/.exec(output);if(match&&external){clearTimeout(timer);resolve({inline:match[1],external:external[1]});}});
      child.stderr.on('data',bytes=>{output=(output+bytes).slice(-16384);});void finished.catch(error=>{clearTimeout(timer);reject(error);});
    });
    return {trackUrl:services.trackUrl,seed:seed.inline,externalSeed:seed.external,diagnostics(){return {exitCode:child.exitCode,output:output.split('\n').filter(line=>!line.includes('Q2Q_BROWSER_SEED')&&!line.includes('Q2Q_BROWSER_EXTERNAL_SEED')).join('\n').slice(-4096)};},async finish(){if(!stopped){stopped=true;child.stdin.end('close\n');}await finished;},async close(){
      if(child.exitCode===null&&child.signalCode===null){
        if(!stopped){stopped=true;child.stdin.end('close\n');}
        const exit=once(child,'exit');
        const timer=setTimeout(()=>child.kill('SIGTERM'),5000);
        const force=setTimeout(()=>child.kill('SIGKILL'),7000);
        try{await exit;}finally{clearTimeout(timer);clearTimeout(force);}
      }
      await services.close();
    }};
  }catch(error){child?.kill('SIGTERM');await services.close();throw error;}
}
