Sources from Linux stable v6.1.190, matching the guest's Debian kernel:

- `gpio-mockup-main.c` — `drivers/gpio/gpio-mockup.c` (renamed so the module can be `gpio-mockup.ko`)
- `irq_sim.c` — `kernel/irq/irq_sim.c`, calling `generic_handle_irq_safe()` instead of
  `handle_simple_irq(irq_to_desc())`, since `irq_to_desc` isn't exported to modules
- `gpiolib.h` — `drivers/gpio/gpiolib.h`

Loaded with `gpio_mockup_ranges=0,28`, giving one chip with lines 0-27 at base 0,
so sysfs GPIO numbers match the Pi's BCM numbers (periph names them `GPIO0`..`GPIO27`).
Inputs are driven through `/sys/kernel/debug/gpio-mockup/gpiochipN/<line>`.
