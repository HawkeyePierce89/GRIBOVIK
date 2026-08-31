export class Counter {
  private value: number;

  constructor(start: number) {
    this.value = start;
  }

  bump(): void {
    this.value = step(this.value);
    this.log();
  }

  private log(): void {}
}

export function step(value: number): number {
  return value + 1;
}
