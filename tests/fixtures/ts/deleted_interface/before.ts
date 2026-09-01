export interface Shape {
  area(): number;
}

export type Name = string;

export function describe(shape: Shape): string {
  return `${shape.area()}`;
}
