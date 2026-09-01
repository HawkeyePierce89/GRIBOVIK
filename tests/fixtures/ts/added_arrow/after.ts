export function greet(name: string): string {
  return `hello, ${name}`;
}

/** Greets everyone, in order. */
export const greetAll = (names: string[]): string[] =>
  names.map((name) => greet(name));
