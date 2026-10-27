import assert from 'node:assert/strict';
import { test } from 'node:test';
import { generateKeyPair, exportJWK, SignJWT, createLocalJWKSet } from 'jose';
import { verifyAccess, AccessError } from '../src/lib/server/access.ts';
const { publicKey, privateKey } = await generateKeyPair('RS256');
const keys = createLocalJWKSet({keys:[{...await exportJWK(publicKey),kid:'test',alg:'RS256'}]});
const config={ACCESS_TEAM_DOMAIN:'test.cloudflareaccess.com',ACCESS_AUD:'admin-audience'};
const now=Math.floor(Date.now()/1000);
async function token(overrides:Record<string,unknown>={}) {
 return new SignJWT({sub:'operator',email:'operator@example.test',type:'app',iss:'https://test.cloudflareaccess.com',aud:'admin-audience',iat:now,exp:now+600,...overrides}).setProtectedHeader({alg:'RS256',kid:'test'}).sign(privateKey);
}
const denied=(status:number)=>(error:unknown)=>error instanceof AccessError && error.status===status;
test('valid Access app token returns only verified display identity',async()=>{
 assert.deepEqual(await verifyAccess(await token({extra:'not exposed'}),config,keys),{id:'https://test.cloudflareaccess.com#operator',email:'operator@example.test'});
});
test('missing or unsafe configuration denies access',async()=>{
 for(const invalid of [{},{...config,ACCESS_AUD:''},{...config,ACCESS_TEAM_DOMAIN:'example.test'},{...config,ACCESS_TEAM_DOMAIN:'test.cloudflareaccess.com.evil.test'}]) await assert.rejects(verifyAccess(null,invalid,keys),denied(503));
});
test('a forged email header cannot replace a missing assertion',async()=>{await assert.rejects(verifyAccess(null,config,keys),denied(401));});
for(const [name,claims] of Object.entries({audience:{aud:'platform-audience'},issuer:{iss:'https://other.cloudflareaccess.com'},expiry:{exp:now-60},subject:{sub:''},email:{email:null},service:{type:'service'},future:{iat:now+600},issuedAt:{iat:null}})) {
 test(`rejects invalid ${name}`,async()=>{await assert.rejects(verifyAccess(await token(claims),config,keys),denied(403));});
}
test('rejects signatures from other keys and algorithms',async()=>{
 const other=await generateKeyPair('RS256');
 const wrong=await new SignJWT({sub:'operator'}).setProtectedHeader({alg:'RS256',kid:'test'}).sign(other.privateKey);
 await assert.rejects(verifyAccess(wrong,config,keys),denied(403));
 const hmac=await new SignJWT({sub:'operator'}).setProtectedHeader({alg:'HS256'}).sign(new Uint8Array(32));
 await assert.rejects(verifyAccess(hmac,config,keys),denied(403));
});
