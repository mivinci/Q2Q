import { execFile, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import { createConnection } from 'node:net';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../../',import.meta.url));
const execute=promisify(execFile);let executable;
export async function nativeCoreTestBinary(){
  executable??=execute('cargo',['test','-p','q2q-core','--lib','--no-run','--message-format=json'],{cwd:root,maxBuffer:8*1024*1024}).then(result=>{
    const artifact=result.stdout.split('\n').filter(Boolean).map(line=>JSON.parse(line)).find(value=>value.reason==='compiler-artifact'&&value.target.name==='q2q_core'&&value.executable);
    if(!artifact)throw Error('native RTC test artifact missing');return artifact.executable;
  });
  return executable;
}
export async function nativeRtcFixture(){
  const child=spawn(await nativeCoreTestBinary(),['browser_rtc_bridge','--ignored','--nocapture'],{cwd:root,env:{...process.env,Q2Q_BROWSER_RTC:'1'},stdio:['ignore','pipe','pipe']});
  let output='';const finished=new Promise((resolve,reject)=>{child.once('exit',code=>code===0?resolve():reject(Error(`native RTC failed ${code}: ${output}`)));child.once('error',reject);});void finished.catch(()=>{});
  let socket;let ended;const queue=[],waiting=[];
  const fail=error=>{ended=error;for(const item of waiting.splice(0)){clearTimeout(item.timer);item.reject(error);}};
  try{
    const port=await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error('native RTC startup timeout')),10000);
      child.stdout.on('data',bytes=>{output=(output+bytes).slice(-8192);const match=/Q2Q_BROWSER_RTC (\d+)/.exec(output);if(match){clearTimeout(timer);resolve(Number(match[1]));}});
      child.stderr.on('data',bytes=>{output=(output+bytes).slice(-8192);});
      child.once('exit',code=>{clearTimeout(timer);const error=Error(`native RTC exited ${code}: ${output}`);reject(error);fail(error);});child.once('error',reject);
    });
    socket=createConnection({host:'127.0.0.1',port});await once(socket,'connect');
    let buffer='';socket.setEncoding('utf8');socket.on('data',chunk=>{
      buffer+=chunk;let end;
      while((end=buffer.indexOf('\n'))>=0){const value=JSON.parse(buffer.slice(0,end));output=(output+'\nwire '+value.type).slice(-8192);buffer=buffer.slice(end+1);const item=waiting.shift();if(item){clearTimeout(item.timer);item.resolve(value);}else queue.push(value);}
    });
    socket.on('error',fail);socket.on('close',()=>fail(Error('native RTC socket closed')));
    const read=()=>{
      if(queue.length)return Promise.resolve(queue.shift());if(ended)return Promise.reject(ended);
      return new Promise((resolve,reject)=>{const item={resolve,reject,timer:setTimeout(()=>{waiting.splice(waiting.indexOf(item),1);reject(Error(`native RTC read timeout: ${output}`));},15000)};waiting.push(item);});
    };
    const identity=await read();if(identity.type!=='identity')throw Error('native RTC identity missing');
    return {publicKey:identity.public_key,read,finished:()=>finished,send(value){if(ended)throw ended;socket.write(JSON.stringify(value)+'\n');},async close(){
      socket.destroy();if(child.exitCode===null){const exit=once(child,'exit');const force=setTimeout(()=>child.kill('SIGKILL'),1000);child.kill('SIGTERM');await exit;clearTimeout(force);}
    }};
  }catch(error){socket?.destroy();child.kill('SIGTERM');throw error;}
}
