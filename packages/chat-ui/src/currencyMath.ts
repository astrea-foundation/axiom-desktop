import remarkMath from "remark-math";
import type { Construct, Extension } from "micromark-util-types";
import type { Processor } from "unified";

function currencyAware(construct: Construct): Construct {
  return {
    ...construct,
    tokenize(effects, ok, nok) {
      const firstEvent = this.events.length;
      return construct.tokenize.call(this, effects, (code) => {
        const token = this.events[firstEvent]?.[1];
        if (!token) return nok(code);
        const source = this.sliceSerialize(token);
        if (!source.startsWith("$$")) {
          const expression = source.slice(1, -1);
          // A closing currency prefix ($10) is not a math delimiter. Requiring
          // content next to each delimiter also keeps "$5 and $10" literal,
          // including while only the second dollar sign has streamed in.
          if (/^\s|\s$/u.test(expression) || (code !== null && code >= 48 && code <= 57)) {
            return nok(code);
          }
        }
        return ok(code);
      }, nok);
    },
  };
}

export function remarkCurrencyMath(this: Processor): void {
  remarkMath.call(this);
  const data = this.data();
  const extensions = "micromarkExtensions" in data ? data.micromarkExtensions as Extension[] : [];
  const extension = extensions?.at(-1);
  const dollar = extension?.text?.[36];
  if (extension?.text && dollar) {
    // Reject ambiguous syntax during tokenization so ordinary Markdown (bold,
    // links, etc.) is still parsed. Code, escapes and display math stay native.
    extension.text[36] = Array.isArray(dollar) ? dollar.map(currencyAware) : currencyAware(dollar);
  }
}
