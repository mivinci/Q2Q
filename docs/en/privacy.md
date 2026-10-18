---
title: Q2Q Platform Privacy Policy
description: How Q2Q Platform handles account, project, and content information.
---

# Q2Q Platform Privacy Policy

Effective date: October 18, 2026

This policy describes how Q2Q Platform at [platform.q2q.app](https://platform.q2q.app) handles information when you use its account, project, storage, seed-processing, and publication features. It does not govern independently operated Track, Punch, storage, or CDN services.

## Information we handle

- **Sign-in information.** When you sign in with Google or GitHub, we receive basic account details made available by that provider, such as your provider account ID, name, email address, and profile image. We use this information to create and maintain your Q2Q account. We do not request access to Google Drive, Gmail, or other unrelated Google services. OAuth credentials needed for sign-in are stored in encrypted form.
- **Account and security information.** We store account and session records. A session can include the IP address and browser user-agent supplied with a sign-in request. We use this information to authenticate sessions, protect the service, and investigate operational issues.
- **Project information.** We store project names, membership and invitation status, selected storage and network settings, and related activity needed to operate project features. Invitation links are represented by a token hash in the database.
- **Files and publication information.** If you upload files, we handle the file contents, names, sizes, upload status, and generated seed and content-proof artifacts. We also store publication details such as content identifiers, configured Track addresses, and download-authorization status.
- **Preferences and cookies.** Necessary cookies maintain your sign-in session and remember your language and selected project. Appearance preferences are stored in your browser. The platform currently does not use advertising cookies or third-party analytics.

If you inspect a `.q2q` seed using the platform's seed inspector, parsing happens in your browser. The inspector does not upload the seed file or visit addresses embedded in it.

## How we use information

We use information to authenticate users; operate accounts, projects, and invitations; store uploaded files; generate seeds and proofs; publish content to a selected distribution network; enforce project access and storage limits; prevent abuse; and maintain the security and reliability of the service.

## Storage and service providers

Q2Q Platform runs on Cloudflare Workers and uses Cloudflare D1, R2, and Queues to store account and project records, uploaded content, and processing tasks. Google and GitHub handle sign-in when you choose those providers. These providers process information under their own terms and privacy policies:

- [Google Privacy Policy](https://policies.google.com/privacy)
- [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement)
- [Cloudflare Privacy Policy](https://www.cloudflare.com/privacypolicy/)

We do not sell personal information or use it for targeted advertising. We may disclose information when required by law or when reasonably necessary to protect users, the service, or others.

## Distribution and information you share

When you publish content or share a seed or download authorization, the information contained in that seed may include content identifiers and the Track or download-source addresses configured for the publication. People who receive a valid seed or authorization may be able to download that version while the authorization remains valid. You can withdraw a publication or revoke an authorization; already-issued links may take up to five minutes to stop working.

Downloaders connect to the Track, Punch, storage, CDN, and peer services named by the publication. Those services and their operators may receive connection information such as IP addresses. Q2Q does not control independently operated services; review their policies before using them.

## Retention and your choices

Sessions expire after seven days unless they are refreshed, and signing out ends the current session. Uploaded files and their generated artifacts remain available to the project until an authorized project member deletes them. Deletion and publication cleanup are processed asynchronously, so removal may not be immediate. We retain account, project, and security records for as long as needed to operate and protect the service, or as otherwise required by law.

The platform does not currently provide self-service account deletion. To ask about, correct, or request deletion of account information, contact [mivinci@foxmail.com](mailto:mivinci@foxmail.com). We may need to verify your identity and consider project ownership, other members' access, security, and applicable legal requirements before fulfilling a request.

## Security

We use measures such as encrypted OAuth credentials, session controls, and project-membership checks to protect information. No internet service can guarantee absolute security; keep your sign-in account and any shared seed or download link secure.

## Changes and contact

We may update this policy as the platform changes. We will publish the latest version on this page and update its effective date. For privacy questions, contact [mivinci@foxmail.com](mailto:mivinci@foxmail.com).
