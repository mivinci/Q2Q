import { test as base } from '@playwright/test';
import { startServer } from './browser-server.mjs';
const test=base.extend({origin:[async({},use)=>{const {server,origin}=await startServer();try{await use(origin);}finally{await new Promise(resolve=>server.close(resolve));}},{scope:'worker'}]});
for(const mismatch of [undefined,'hash','key','connection'])test(`RTC identity and binary exchange ${mismatch??'valid'} @rtc`,async({page,origin})=>{
  await page.goto(origin);await page.evaluate(async mismatch=>(await import('/tests/rtc.browser.ts')).pair({mismatch}),mismatch);
});
test('closing RTC during negotiation rejects pending ready @rtc',async({page,origin})=>{
  await page.goto(origin);await page.evaluate(async()=>(await import('/tests/rtc.browser.ts')).pair({cancel:true}));
});
test('RTC negotiation has a deadline and closes @rtc',async({page,origin})=>{
  await page.goto(origin);await page.evaluate(async()=>(await import('/tests/rtc.browser.ts')).timeout());
});

for(const nativeInitiator of [false,true])test(`browser and native webrtc-rs interoperate, native initiator=${nativeInitiator} @rtc`,async({page,origin,browserName})=>{
  test.skip(browserName==='webkit'&&process.env.TEST_NATIVE_WEBKIT!=='1','WebKit/native ICE remains unverified; TEST_NATIVE_WEBKIT=1 reruns the diagnostic');
  const {nativeRtcFixture}=await import('./native-rtc-fixture.mjs');const native=await nativeRtcFixture();
  try{
    await page.exposeFunction('nativeRead',()=>native.read());await page.exposeFunction('nativeSend',value=>native.send(value));
    await page.goto(origin);
    await page.evaluate(async({key,initiator})=>(await import('/tests/rtc.browser.ts')).nativeExchange(key,initiator),{key:native.publicKey,initiator:nativeInitiator});
    await native.finished();
  }finally{await native.close();}
});

for(const option of ['pressure','overflow','smallMessageSize'])test(`RTC bounded host behavior ${option} @rtc`,async({page,origin})=>{
  await page.goto(origin);await page.evaluate(async option=>(await import('/tests/rtc.browser.ts')).pair({[option]:true}),option);
});

for(const mode of ['timeout','candidate'])test(`RTC diagnostic captures ${mode} before cleanup @rtc`,async({page,origin})=>{
 await page.goto(origin);await page.evaluate(async mode=>(await import('/tests/rtc.browser.ts')).diagnosticFailure(mode),mode);
});
