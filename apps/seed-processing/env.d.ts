// Secrets are configured per Worker and may be absent in local or CI environments.
declare namespace Cloudflare {
  interface Env {
    S3_ACCESS_KEY_ID?: string;
    S3_SECRET_ACCESS_KEY?: string;
    S3_FORCE_PATH_STYLE?: string;
  }
}
