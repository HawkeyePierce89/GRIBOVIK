export namespace Api {
  export function fetchUser(id: string): string {
    return normalize(id);
  }

  export class Client {
    send(body: string): string {
      return body;
    }
  }
}

declare module "legacy" {
  export function shim(): void;
}

export function main(): string {
  return Api.fetchUser("1");
}
