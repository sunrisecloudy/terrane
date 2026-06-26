export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const request = new XMLHttpRequest();
  request.open("GET", "https://example.com");
  request.send();
  return null;
}
