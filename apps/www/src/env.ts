import {defineEnvVars} from '@sveltejs/kit/env';
export const variables=defineEnvVars({
 PUBLIC_Q2Q_TRACK_URL:{public:true,static:true,schema:(value:string|undefined)=>{
  const configured=value??'https://track.q2q.app';
  if(configured==='')return '';
  const url=new URL(configured);
  if(url.username||url.password||url.search||url.hash||!(url.protocol==='https:'||url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('Invalid public Track URL');
  return url.href.replace(/\/$/,'');
 }}
});
