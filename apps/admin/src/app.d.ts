import type { Locale } from '#lib/messages.js';
declare global {
 namespace Cloudflare { interface Env { ACCESS_TEAM_DOMAIN?: string; ACCESS_AUD?: string;  } }
 namespace App {
  interface Locals { admin: { id: string; email: string }; locale: Locale }
  interface PageData { locale: Locale; admin: { id: string; email: string } }
 }
}
export {};
