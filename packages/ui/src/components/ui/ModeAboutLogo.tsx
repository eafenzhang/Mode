import { cn } from "@/components/lib/utils.js";

export function ModeAboutLogo({ className }: { className?: string }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      width="118"
      height="102"
      fill="none"
      viewBox="190 258 644 556"
      className={cn("shrink-0 text-current", className)}
      aria-hidden="true"
      focusable="false"
    >
      {/* Mode 品牌标记：与应用图标同款的渐变 M。 */}
      <defs>
        <linearGradient
          id="mode-brand-m-gradient"
          x1="756"
          y1="336"
          x2="300"
          y2="736"
          gradientUnits="userSpaceOnUse"
        >
          <stop offset="0" stopColor="#38bdf8" />
          <stop offset="1" stopColor="#818cf8" />
        </linearGradient>
      </defs>
      <path
        d="M268 736V336L512 592L756 336V736"
        fill="none"
        stroke="url(#mode-brand-m-gradient)"
        strokeWidth="132"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
