import type { Locale } from '#lib/i18n.js';
// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
  namespace Cloudflare {
    interface Env {
      PROCESSING_QUEUE?: Queue;
      STORAGE?: R2Bucket;
      UPLOAD_LOCAL_PROXY?: string;
      S3_ENDPOINT?: string;
      S3_REGION?: string;
      S3_BUCKET?: string;
      S3_ACCESS_KEY_ID?: string;
      S3_SECRET_ACCESS_KEY?: string;
      S3_FORCE_PATH_STYLE?: string;
      AUTH_ORIGIN?: string;
      BETTER_AUTH_SECRET?: string;
      GITHUB_CLIENT_ID?: string;
      GITHUB_CLIENT_SECRET?: string;
      BILLING_SIMULATION?: string;
    }
  }

	namespace App {


		// interface Error {}
		interface Locals { locale: Locale; authReady: boolean; user: { id: string; name: string; image: string | null } | null }
		interface PageData { locale: Locale }
		// interface PageState {}
	}
}

export {};
