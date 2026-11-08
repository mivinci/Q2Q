export const load = ({ cookies }: { cookies: { get(name:string): string | undefined } }) => ({
  locale: cookies.get('q2q-web-locale') === 'en' ? 'en' as const : 'zh-CN' as const
});
