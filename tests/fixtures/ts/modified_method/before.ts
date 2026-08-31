export class Counter {
  private value: number;

  constructor(start: number) {
    this.value = start;
  }

  bump(): void {
    this.value += 1;
  }
}
