const LOCAL_BROWSER_HOSTNAMES = new Set([
  "0.0.0.0",
  "127.0.0.1",
  "::",
  "::1",
  "localhost",
]);

export function browserReachableLocalUrl(localUrl: string): string {
  try {
    const url = new URL(localUrl);
    if (LOCAL_BROWSER_HOSTNAMES.has(url.hostname.toLowerCase())) {
      url.hostname = window.location.hostname;
    }
    return url.toString();
  } catch {
    return localUrl;
  }
}