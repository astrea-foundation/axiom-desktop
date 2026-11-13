export interface SakuraColor {
  gradientColorStart: string;
  gradientColorEnd: string;
  gradientColorDegree: number;
}

export interface SakuraOptions {
  className?: string;
  fallSpeed?: number;
  maxSize?: number;
  minSize?: number;
  delay?: number;
  originBiasRight?: number;
  colors?: SakuraColor[];
}

type Settings = Required<SakuraOptions>;

const DEFAULTS: Settings = {
  className: "sakura",
  fallSpeed: 1,
  maxSize: 14,
  minSize: 10,
  delay: 300,
  originBiasRight: 0,
  colors: [
    {
      gradientColorStart: "rgba(255, 183, 197, 0.9)",
      gradientColorEnd: "rgba(255, 197, 208, 0.9)",
      gradientColorDegree: 120,
    },
  ],
};

const BLOW_ANIMATIONS = [
  "blow-soft-left",
  "blow-medium-left",
  "blow-soft-right",
  "blow-medium-right",
];
const SWAY_ANIMATIONS = [
  "sway-0",
  "sway-1",
  "sway-2",
  "sway-3",
  "sway-4",
  "sway-5",
  "sway-6",
  "sway-7",
  "sway-8",
];
const ANIM_PREFIXES = ["webkit", "moz", "MS", "o", ""];

function randomArrayElem<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)] as T;
}

function randomInt(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function prefixedEvent(el: Element, type: string, callback: () => void): void {
  for (const prefix of ANIM_PREFIXES) {
    el.addEventListener(prefix ? prefix + type : type.toLowerCase(), callback, false);
  }
}

function elementInViewport(el: Element): boolean {
  const rect = el.getBoundingClientRect();
  return (
    rect.top >= 0 &&
    rect.left >= 0 &&
    rect.bottom <= (window.innerHeight || document.documentElement.clientHeight) &&
    rect.right <= (window.innerWidth || document.documentElement.clientWidth)
  );
}

export default class Sakura {
  private readonly el: HTMLElement;
  private readonly settings: Settings;
  private running = false;
  private rafId: number | null = null;
  private timeoutId: number | null = null;

  constructor(target: string | HTMLElement, options: SakuraOptions = {}) {
    const el = typeof target === "string" ? document.querySelector<HTMLElement>(target) : target;
    if (!el) {
      throw new Error("Sakura: target element not found.");
    }
    this.el = el;
    this.settings = { ...DEFAULTS, ...options };
    this.el.style.overflowX = "hidden";
    this.start();
  }

  private createPetal = (): void => {
    if (!this.running) return;

    this.timeoutId = window.setTimeout(() => {
      this.rafId = window.requestAnimationFrame(this.createPetal);
    }, this.settings.delay);

    const blowAnimation = randomArrayElem(BLOW_ANIMATIONS);
    const swayAnimation = randomArrayElem(SWAY_ANIMATIONS);
    const fallTime =
      (document.documentElement.clientHeight * 0.007 + Math.round(Math.random() * 5)) *
      this.settings.fallSpeed;

    const animations = [
      `fall ${fallTime}s linear 0s 1`,
      `${blowAnimation} ${(fallTime > 30 ? fallTime : 30) - 20 + randomInt(0, 20)}s linear 0s infinite`,
      `${swayAnimation} ${randomInt(2, 4)}s linear 0s infinite`,
    ].join(", ");

    const petal = document.createElement("div");
    petal.classList.add(this.settings.className);
    const height = randomInt(this.settings.minSize, this.settings.maxSize);
    const width = height - Math.floor(randomInt(0, this.settings.minSize) / 3);
    const color = randomArrayElem(this.settings.colors);

    petal.style.background = `linear-gradient(${color.gradientColorDegree}deg, ${color.gradientColorStart}, ${color.gradientColorEnd})`;
    petal.style.animation = animations;
    petal.style.borderRadius = `${randomInt(
      this.settings.maxSize,
      this.settings.maxSize + Math.floor(Math.random() * 10),
    )}px ${randomInt(1, Math.floor(width / 4))}px`;
    petal.style.height = `${height}px`;
    const spawnWidth = this.el.clientWidth || document.documentElement.clientWidth;
    let fx = Math.random();
    if (this.settings.originBiasRight > 0) {
      const skew = 1.0 - Math.random() * Math.random();
      fx = fx * (1 - this.settings.originBiasRight) + skew * this.settings.originBiasRight;
    }
    petal.style.left = `${fx * spawnWidth - 100}px`;
    petal.style.marginTop = `${-(Math.floor(Math.random() * 20) + 15)}px`;
    petal.style.width = `${width}px`;

    prefixedEvent(petal, "AnimationEnd", () => {
      if (!elementInViewport(petal)) petal.remove();
    });
    prefixedEvent(petal, "AnimationIteration", () => {
      if (!elementInViewport(petal)) petal.remove();
    });

    this.el.appendChild(petal);
  };

  start(): void {
    if (this.running) return;
    this.running = true;
    this.rafId = window.requestAnimationFrame(this.createPetal);
  }

  stop(graceful = false): void {
    this.running = false;
    if (this.rafId !== null) {
      window.cancelAnimationFrame(this.rafId);
      this.rafId = null;
    }
    if (this.timeoutId !== null) {
      window.clearTimeout(this.timeoutId);
      this.timeoutId = null;
    }

    if (!graceful) {
      window.setTimeout(() => {
        const petals = this.el.getElementsByClassName(this.settings.className);
        while (petals.length > 0) {
          petals[0]?.parentNode?.removeChild(petals[0]);
        }
      }, this.settings.delay + 50);
    }
  }
}
