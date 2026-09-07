import * as React from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Check, Search } from "lucide-react";
import { cn } from "@/lib/utils";

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
  keywords?: readonly string[];
}

type SelectProps = {
  options: readonly SelectOption[];
  className?: string;
  id?: string;
  disabled?: boolean;
  searchable?: boolean;
  searchPlaceholder?: string;
  emptyMessage?: string;
  placeholder?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: React.AriaAttributes["aria-invalid"];
} & ({
  multiple?: false;
  value: string;
  onChange: (val: string) => void;
} | {
  multiple: true;
  value: string[];
  onChange: (val: string[]) => void;
});

export function Select({
  value,
  onChange,
  multiple,
  options,
  className,
  id,
  disabled = false,
  searchable = false,
  searchPlaceholder = "搜索选项",
  emptyMessage = "没有匹配的选项",
  placeholder = "请选择",
  "aria-describedby": ariaDescribedBy,
  "aria-invalid": ariaInvalid,
}: SelectProps) {
  const [open, setOpen] = React.useState(false);
  const [activeIndex, setActiveIndex] = React.useState(0);
  const [searchQuery, setSearchQuery] = React.useState("");
  const containerRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  const dropdownRef = React.useRef<HTMLDivElement>(null);
  const searchRef = React.useRef<HTMLInputElement>(null);
  const optionRefs = React.useRef<Array<HTMLButtonElement | null>>([]);
  const [dropdownStyle, setDropdownStyle] = React.useState<React.CSSProperties>({});
  const generatedId = React.useId();
  const triggerId = id ?? generatedId;
  const listboxId = `${triggerId}-listbox`;
  const filteredOptions = React.useMemo(() => {
    const query = searchQuery.trim().normalize("NFKC").toLocaleLowerCase();
    if (!searchable || !query) return options;
    return options.filter((option) =>
      [option.label, option.value, option.description, ...(option.keywords ?? [])]
        .filter(Boolean)
        .some((item) => item?.normalize("NFKC").toLocaleLowerCase().includes(query)),
    );
  }, [options, searchQuery, searchable]);
  const isValueSelected = (optionValue: string) => multiple ? value.includes(optionValue) : value === optionValue;
  const matchedSelectedIndex = filteredOptions.findIndex((option) => isValueSelected(option.value));
  const selectedIndex = Math.max(0, matchedSelectedIndex);

  function updateDropdownPosition() {
    const container = containerRef.current;
    if (!container) return;
    const rect = container.getBoundingClientRect();
    const viewportMargin = 8;
    const dropdownGap = 6;
    const maximumHeight = 240;
    const availableBelow = window.innerHeight - rect.bottom - dropdownGap - viewportMargin;
    const availableAbove = rect.top - dropdownGap - viewportMargin;
    const openAbove = availableBelow < Math.min(160, maximumHeight) && availableAbove > availableBelow;
    const width = Math.min(rect.width, window.innerWidth - viewportMargin * 2);
    const left = Math.min(
      Math.max(rect.left, viewportMargin),
      Math.max(viewportMargin, window.innerWidth - viewportMargin - width),
    );
    setDropdownStyle({
      position: "fixed",
      top: openAbove ? undefined : rect.bottom + dropdownGap,
      bottom: openAbove ? window.innerHeight - rect.top + dropdownGap : undefined,
      left,
      width,
      maxHeight: Math.max(72, Math.min(maximumHeight, openAbove ? availableAbove : availableBelow)),
    });
  }

  function openDropdown(index = selectedIndex) {
    if (disabled || options.length === 0) return;
    updateDropdownPosition();
    setSearchQuery("");
    setActiveIndex(Math.min(Math.max(index, 0), options.length - 1));
    setOpen(true);
  }

  function closeDropdown({ restoreFocus = false } = {}) {
    setOpen(false);
    setSearchQuery("");
    if (restoreFocus) {
      requestAnimationFrame(() => triggerRef.current?.focus());
    }
  }

  function selectValue(optionValue: string) {
    if (multiple) {
      onChange(value.includes(optionValue) ? value.filter((item) => item !== optionValue) : [...value, optionValue]);
    } else {
      onChange(optionValue);
      closeDropdown({ restoreFocus: true });
    }
  }

  function focusOption(index: number) {
    if (filteredOptions.length === 0) return;
    const nextIndex = (index + filteredOptions.length) % filteredOptions.length;
    setActiveIndex(nextIndex);
    optionRefs.current[nextIndex]?.focus();
  }

  function focusAdjacentToTrigger(backward: boolean) {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const scope = trigger.closest<HTMLElement>('[role="dialog"]') ?? document.body;
    const focusable = Array.from(scope.querySelectorAll<HTMLElement>(
      'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
    )).filter((element) => element.offsetParent !== null && element.tabIndex >= 0);
    const triggerIndex = focusable.indexOf(trigger);
    if (triggerIndex < 0 || focusable.length === 0) {
      trigger.focus();
      return;
    }
    const offset = backward ? -1 : 1;
    focusable[(triggerIndex + offset + focusable.length) % focusable.length]?.focus();
  }

  React.useEffect(() => {
    function handleClickOutside(e: MouseEvent) {
      const target = e.target as Node;
      if (
        containerRef.current &&
        !containerRef.current.contains(target) &&
        dropdownRef.current &&
        !dropdownRef.current.contains(target)
      ) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  React.useEffect(() => {
    if (!open) return;
    function handleScroll(e: Event) {
      if (dropdownRef.current && dropdownRef.current.contains(e.target as Node)) {
        return;
      }
      // Multi-select can change the page height while its menu stays open.
      // Follow the trigger during the resulting scroll instead of dismissing it.
      if (multiple) {
        const rect = triggerRef.current?.getBoundingClientRect();
        if (rect && rect.bottom > 0 && rect.top < window.innerHeight) {
          updateDropdownPosition();
          return;
        }
      }
      setOpen(false);
    }
    window.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleScroll);
    return () => {
      window.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("resize", handleScroll);
    };
  }, [open, multiple]);

  React.useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  React.useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => {
      if (searchable) searchRef.current?.focus({ preventScroll: true });
      else optionRefs.current[activeIndex]?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [open, searchable]);

  React.useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, filteredOptions.length - 1)));
  }, [filteredOptions.length]);

  const selectedOption = options.find((option) => option.value === value);
  const selectedLabel = multiple
    ? value.length ? value.map((item) => options.find((option) => option.value === item)?.label ?? item).join("、") : placeholder
    : selectedOption?.label ?? (value ? "当前选项不可用" : placeholder);

  return (
    <div ref={containerRef} className={cn("relative", className)}>
      <button
        ref={triggerRef}
        id={triggerId}
        type="button"
        disabled={disabled}
        aria-describedby={ariaDescribedBy}
        aria-invalid={ariaInvalid}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-controls={listboxId}
        aria-owns={open ? listboxId : undefined}
        onClick={() => (open ? closeDropdown() : openDropdown())}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") {
            event.preventDefault();
            openDropdown(selectedIndex);
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            openDropdown(matchedSelectedIndex >= 0 ? selectedIndex : options.length - 1);
          } else if (event.key === "Escape" && open) {
            event.preventDefault();
            event.stopPropagation();
            closeDropdown({ restoreFocus: true });
          }
        }}
        className="flex h-11 w-full items-center justify-between rounded-lg border border-border bg-input px-4 py-2 text-sm transition-colors hover:bg-accent/50 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-card aria-[invalid=true]:border-destructive disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-input"
      >
        <span className="truncate" title={selectedLabel}>{selectedLabel}</span>
        {multiple && value.length > 0 ? <span className="ml-2 shrink-0 text-xs tabular-nums">已选 {value.length}</span> : null}
        <ChevronDown className="ml-2 h-4 w-4 shrink-0 opacity-50" aria-hidden="true" />
      </button>

      {open && !disabled && typeof document !== "undefined" && createPortal(
        <div
          ref={dropdownRef}
          data-dialog-focus-portal="true"
          style={dropdownStyle}
          className="absolute z-[100] flex max-h-72 flex-col overflow-hidden rounded-lg border border-border bg-card shadow-lg animate-in fade-in-0 zoom-in-95"
          onClick={(event) => event.stopPropagation()}
        >
          {searchable ? (
            <div className="border-b border-border p-2">
              <div className="relative">
                <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted" aria-hidden="true" />
                <input
                  ref={searchRef}
                  type="search"
                  value={searchQuery}
                  onChange={(event) => {
                    setSearchQuery(event.target.value);
                    setActiveIndex(0);
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "ArrowDown") {
                      event.preventDefault();
                      focusOption(0);
                    } else if (event.key === "ArrowUp") {
                      event.preventDefault();
                      focusOption(filteredOptions.length - 1);
                    } else if (event.key === "Enter" && filteredOptions.length === 1) {
                      event.preventDefault();
                      selectValue(filteredOptions[0].value);
                    } else if (event.key === "Escape") {
                      event.preventDefault();
                      event.stopPropagation();
                      closeDropdown({ restoreFocus: true });
                    } else if (event.key === "Tab") {
                      event.preventDefault();
                      closeDropdown();
                      requestAnimationFrame(() => focusAdjacentToTrigger(event.shiftKey));
                    }
                  }}
                  placeholder={searchPlaceholder}
                  aria-label={searchPlaceholder}
                  aria-controls={listboxId}
                  className="h-10 w-full rounded-xl border border-border bg-input pl-9 pr-3 text-sm outline-none transition-colors placeholder:text-muted focus:border-primary focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-card"
                />
              </div>
            </div>
          ) : null}
          <div id={listboxId} role="listbox" aria-multiselectable={multiple || undefined} aria-labelledby={triggerId} className="min-h-0 overflow-y-auto p-1">
          {filteredOptions.length === 0 ? (
            <p className="px-3 py-8 text-center text-sm text-muted">{emptyMessage}</p>
          ) : filteredOptions.map((opt, index) => {
            const isSelected = isValueSelected(opt.value);
            return (
              <button
                key={opt.value}
                ref={(element) => {
                  optionRefs.current[index] = element;
                }}
                type="button"
                role="option"
                aria-selected={isSelected}
                tabIndex={activeIndex === index ? 0 : -1}
                className={cn(
                  "flex w-full items-center justify-between rounded-xl px-3 py-2.5 text-sm transition-colors",
                  isSelected
                    ? "bg-primary font-semibold text-primary-foreground shadow-glow"
                    : "text-foreground hover:bg-accent",
                )}
                onClick={() => {
                  selectValue(opt.value);
                }}
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    focusOption(index + 1);
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    focusOption(index - 1);
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    focusOption(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    focusOption(filteredOptions.length - 1);
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    closeDropdown({ restoreFocus: true });
                  } else if (event.key === "Tab") {
                    event.preventDefault();
                    event.stopPropagation();
                    closeDropdown();
                    requestAnimationFrame(() => focusAdjacentToTrigger(event.shiftKey));
                  }
                }}
              >
                <span className="min-w-0 text-left">
                  <span className="block truncate">{opt.label}</span>
                  {opt.description ? <span className="mt-0.5 block truncate text-[11px] opacity-70">{opt.description}</span> : null}
                </span>
                {isSelected && <Check className="ml-2 h-4 w-4 shrink-0" />}
              </button>
            );
          })}
          </div>
        </div>,
        document.body
      )}
    </div>
  );
}
