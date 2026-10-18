// Tutto 品牌标记（与应用图标同款的几何 T，spec: docs/specs/p3-tutto-naming.md）。
import { cn } from "@/components/lib/utils.js";

export function TuttoLogo({ className }: { className?: string }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="118"
      height="118"
      fill="none"
      viewBox="0 0 1024 1024"
      className={cn("shrink-0 text-current", className)}
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <linearGradient
          id="tutto-brand-t-gradient"
          x1="252"
          y1="292"
          x2="772"
          y2="820"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#27E1B5" />
          <stop offset="1" stopColor="#4C8DFF" />
        </linearGradient>
      </defs>
      <path
        d="M252 292H772V412H574V752L512 820L450 752V412H252Z"
        fill="url(#tutto-brand-t-gradient)"
      />
    </svg>
  );
}
