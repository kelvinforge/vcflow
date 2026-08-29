// Minimal primitives matching the existing utilitarian dark style. Not a
// component library -- just enough shared markup to keep panels consistent.

import {
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type ReactNode,
  type TextareaHTMLAttributes,
} from "react"
import { ChevronDown, ChevronRight, Copy } from "lucide-react"
import { cn } from "@/lib/utils"

type Variant = "primary" | "secondary" | "destructive"

const VARIANTS: Record<Variant, string> = {
  primary: "bg-primary text-primary-foreground",
  secondary: "bg-secondary text-secondary-foreground",
  destructive: "bg-destructive text-destructive-foreground",
}

export function Button({
  variant = "secondary",
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      className={cn(
        "rounded px-3 py-1 text-sm disabled:opacity-50 disabled:cursor-not-allowed",
        VARIANTS[variant],
        className,
      )}
      {...props}
    />
  )
}

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "rounded border border-border bg-transparent px-2 py-1 text-sm",
        className,
      )}
      {...props}
    />
  )
}

export function Textarea({ className, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        "min-h-16 rounded border border-border bg-transparent px-2 py-1 text-sm",
        className,
      )}
      {...props}
    />
  )
}

export function Card({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <div className={cn("rounded-lg border border-border bg-card p-4", className)}>{children}</div>
  )
}

export function Badge({
  tone = "muted",
  children,
  title,
}: {
  tone?: "muted" | "ok" | "warn" | "bad"
  children: ReactNode
  title?: string
}) {
  const tones = {
    muted: "bg-muted text-muted-foreground",
    ok: "bg-primary/15 text-primary",
    warn: "bg-destructive/10 text-destructive",
    bad: "bg-destructive/20 text-destructive",
  }
  return (
    <span title={title} className={cn("rounded px-1.5 py-0.5 text-xs font-medium", tones[tone])}>
      {children}
    </span>
  )
}

/** Collapsible section. Secondary panels live inside these; the focus card does not. */
export function Section({
  title,
  right,
  defaultOpen = false,
  children,
}: {
  title: string
  right?: ReactNode
  defaultOpen?: boolean
  children: ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <div className="rounded-lg border border-border bg-card">
      <div className="flex items-center justify-between px-3 py-2">
        <button
          className="flex items-center gap-1 text-sm font-medium text-foreground"
          onClick={() => setOpen((o) => !o)}
        >
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          {title}
        </button>
        {right}
      </div>
      {open && <div className="border-t border-border px-3 py-3">{children}</div>}
    </div>
  )
}

/** Raw backend error string + Copy + optional Retry. Never parsed. */
export function ErrorLine({ error, onRetry }: { error: string; onRetry?: () => void }) {
  const [copied, setCopied] = useState(false)
  return (
    <div className="flex flex-col gap-1 rounded border border-destructive/40 bg-destructive/5 p-2 text-xs">
      <pre className="whitespace-pre-wrap break-words text-destructive">{error}</pre>
      <div className="flex gap-2">
        <button
          className="flex items-center gap-1 text-muted-foreground hover:text-foreground"
          onClick={() => {
            navigator.clipboard.writeText(error).then(() => {
              setCopied(true)
              setTimeout(() => setCopied(false), 1500)
            })
          }}
        >
          <Copy size={12} /> {copied ? "Copied" : "Copy"}
        </button>
        {onRetry && (
          <button className="text-muted-foreground hover:text-foreground" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
    </div>
  )
}
