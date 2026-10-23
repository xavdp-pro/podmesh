import { ArrowUpRight, BookOpen, CheckCircle2, ChevronDown, Circle, Download, TriangleAlert } from 'lucide-react'
import { Link } from 'react-router-dom'
import snapshot from '../data/validationSnapshot.json'
import snapshotUrl from '../data/validationSnapshot.json?url'
import { validationCopy } from '../data/validationCopy'

export function coverage(feature) {
  const validated = feature.checks.filter((criterion) => criterion.status === 'validated').length
  return { validated, total: feature.checks.length, percentage: Math.round(100 * validated / feature.checks.length) }
}

export default function ValidationPage({ locale = 'fr' }) {
  const lang = validationCopy[locale] ? locale : 'fr'
  const copy = validationCopy[lang]
  const timestamp = snapshot.updatedAtUTC || `${snapshot.updatedAt}T12:00:00Z`
  const date = new Intl.DateTimeFormat(lang, { day: 'numeric', month: 'long', year: 'numeric', hour: '2-digit', minute: '2-digit', timeZone: 'Europe/Paris' }).format(new Date(timestamp))

  return (
    <article id="validation" tabIndex={-1} className="scroll-mt-24 space-y-9">
      <header className="border-b border-line pb-8">
        <p className="text-sm font-semibold text-accent">{copy.eyebrow}</p>
        <h1 className="site-display mt-4 max-w-3xl text-4xl font-semibold leading-tight sm:text-5xl">{copy.title}</h1>
        <p className="mt-5 max-w-3xl text-base leading-7 text-fg-muted sm:text-lg sm:leading-8">{copy.intro}</p>
        <div className="mt-5 flex flex-wrap gap-x-5 gap-y-2 text-sm text-fg-faint">
          <p>{copy.updated} <time dateTime={timestamp}>{date}</time> · {copy.timezone}</p>
          <p>{copy.manual}</p>
        </div>
        <p className="mt-3 max-w-3xl text-sm leading-6 text-fg-muted">{copy.updatePolicy}</p>
      </header>

      <aside className="flex gap-3 rounded-xl border border-warm/30 bg-warm-soft p-5" aria-label={copy.productionTitle}>
        <TriangleAlert size={22} className="mt-0.5 shrink-0 text-warm" aria-hidden="true" />
        <div><p className="font-semibold">{copy.productionTitle}</p><p className="mt-2 text-sm leading-6 text-fg-muted">{copy.productionBody}</p></div>
      </aside>

      {snapshot.currentMilestone && <section className="rounded-xl border border-warm/30 bg-warm-soft p-5" aria-labelledby="current-milestone-title" data-current-milestone>
        <h2 id="current-milestone-title" className="text-lg font-semibold">{snapshot.currentMilestone.title[lang]}</h2>
        <p className="mt-3 text-sm leading-7 text-fg-muted">{snapshot.currentMilestone.summary[lang]}</p>
        <p className="mt-3 text-sm leading-7 text-fg-muted">{snapshot.currentMilestone.next[lang]}</p>
        <Link to={`/validation#proof-${snapshot.currentMilestone.proof}`} className="mt-4 inline-flex items-center gap-1 text-sm font-semibold text-accent underline underline-offset-4">{copy.proof}<ArrowUpRight size={14} aria-hidden="true" /></Link>
      </section>}

      {snapshot.canonicalBoundary && <section className="rounded-xl border border-warm/30 p-5" aria-labelledby="canonical-title" data-canonical-boundary>
        <h2 id="canonical-title" className="text-lg font-semibold">{snapshot.canonicalBoundary.title[lang]}</h2>
        <p className="mt-3 text-sm leading-7 text-fg-muted">{snapshot.canonicalBoundary.summary[lang]}</p>
        <ul className="mt-3 list-disc space-y-2 pl-5 text-sm leading-7 text-fg-muted">{snapshot.canonicalBoundary.gaps[lang].map((gap) => <li key={gap}>{gap}</li>)}</ul>
        <Link to="/validation#proof-canonical-boundary" className="mt-4 inline-flex items-center gap-1 text-sm font-semibold text-accent underline underline-offset-4">{copy.proof}<ArrowUpRight size={14} aria-hidden="true" /></Link>
      </section>}

      <section id="validation-criteria" tabIndex={-1} className="scroll-mt-24" aria-labelledby="coverage-title">
        <div className="mb-4 flex flex-wrap items-baseline justify-between gap-3">
          <h2 id="coverage-title" className="site-display text-2xl font-semibold">{copy.coverage}</h2>
          <Link to="/validation#validation-method" className="inline-flex items-center gap-1 text-sm font-semibold text-accent underline underline-offset-4">{copy.navMethod}<ArrowUpRight size={14} aria-hidden="true" /></Link>
        </div>
        <div className="divide-y divide-line border-y border-line">
          {snapshot.features.map((feature) => {
            const score = coverage(feature)
            const blocking = feature.checks.some((criterion) => criterion.status === 'blocked')
            return (
              <section key={feature.id} id={`validation-${feature.id}`} className="scroll-mt-24 py-7" data-validation-feature={feature.id}>
                <div className="flex items-start justify-between gap-4">
                  <div className="min-w-0">
                    <h3 className="text-xl font-semibold">{feature.name[lang]}</h3>
                    <p className="mt-1 text-xs leading-5 text-fg-faint">{feature.scope[lang]}</p>
                    <p className="mt-3 text-sm leading-6 text-fg-muted">{feature.intro[lang]}</p>
                  </div>
                  <div className="shrink-0 text-right">
                    <p className="mb-2 max-w-36 text-xs leading-5 text-fg-muted" data-coverage-scope>{feature.qualificationLabel[lang]}</p>
                    <p className={`text-3xl font-semibold tabular-nums ${blocking ? 'text-warm' : 'text-accent'}`} data-coverage-percent>{score.percentage}<span className="ml-0.5 text-lg">%</span></p>
                    <p className="mt-1 text-xs tabular-nums text-fg-faint">{score.validated} / {score.total}</p>
                  </div>
                </div>
                <div className="mt-5 rounded-xl border border-accent/20 bg-accent/5 p-4 sm:p-5" data-lab-acquired>
                  <h4 className="flex items-center gap-2 text-sm font-semibold text-accent"><CheckCircle2 size={17} aria-hidden="true" />{copy.labAcquired}</h4>
                  <p className="mt-2 text-sm leading-7 text-fg-muted">{feature.labSummary[lang]}</p>
                  <div className="mt-3 flex flex-wrap gap-x-4 gap-y-2">
                    {feature.labProofs.map((id) => {
                      const proof = snapshot.proofs.find((item) => item.id === id)
                      return <Link key={id} to={`/validation#proof-${id}`} className="inline-flex items-center gap-1 text-xs font-semibold text-accent underline underline-offset-4"><BookOpen size={13} aria-hidden="true" />{proof.title[lang]}</Link>
                    })}
                  </div>
                </div>
                {feature.qualificationNote && <p className="mt-4 text-sm font-medium leading-6 text-warm" data-qualification-note>{feature.qualificationNote[lang]}</p>}
                <progress className={`validation-progress mt-4 h-2 w-full ${blocking ? 'text-warm' : 'text-accent'}`} value={score.validated} max={score.total} aria-label={`${feature.name[lang]} : ${score.validated} / ${score.total} ${copy.counted}`} />
                <div className="mt-2 flex flex-wrap justify-between gap-2 text-xs">
                  <p className={blocking ? 'font-semibold text-warm' : 'font-medium text-accent'}>{feature.state[lang]}</p>
                  <p className="text-fg-faint">{score.validated} / {score.total} {copy.counted}</p>
                </div>
                <details className="group mt-4 rounded-xl border border-line bg-surface px-4 py-3 sm:px-5">
                  <summary className="flex cursor-pointer items-center gap-2 text-sm font-semibold">{copy.details}<span className="text-fg-faint">({score.total})</span><ChevronDown size={16} className="ml-auto shrink-0 group-open:rotate-180" aria-hidden="true" /></summary>
                  <ul className="mt-4 divide-y divide-line">
                    {feature.checks.map((criterion) => {
                      const passed = criterion.status === 'validated'
                      const Icon = passed ? CheckCircle2 : criterion.status === 'blocked' ? TriangleAlert : Circle
                      return (
                        <li key={criterion.id} className="flex items-start gap-3 py-3" data-criterion-status={criterion.status}>
                          <Icon size={17} className={`mt-1 shrink-0 ${passed ? 'text-accent' : criterion.status === 'blocked' ? 'text-warm' : 'text-fg-faint'}`} aria-hidden="true" />
                          <div className="min-w-0 flex-1"><p className="text-sm leading-6">{criterion.label[lang]}</p><p className="mt-1 text-xs text-fg-faint">{copy.status[criterion.status]}</p></div>
                          {criterion.proof && <Link to={`/validation#proof-${criterion.proof}`} className="mt-1 inline-flex shrink-0 items-center gap-1 text-xs text-accent underline underline-offset-4" aria-label={`${copy.proof} : ${criterion.label[lang]}`}><BookOpen size={13} aria-hidden="true" /><span className="hidden sm:inline">{copy.proof}</span></Link>}
                        </li>
                      )
                    })}
                  </ul>
                </details>
                <p className="mt-3 border-l-2 border-line pl-4 text-sm leading-6 text-fg-muted"><span className="font-semibold text-fg">{copy.remaining} : </span>{feature.remaining[lang]}</p>
              </section>
            )
          })}
        </div>
      </section>

      <section id="validation-method" tabIndex={-1} className="scroll-mt-24 rounded-2xl border border-line bg-surface p-5 sm:p-7">
        <h2 className="site-display text-2xl font-semibold">{copy.methodology}</h2>
        <ol className="mt-5 list-decimal space-y-3 pl-5 text-sm leading-7 text-fg-muted">{copy.method.map((item) => <li key={item}>{item}</li>)}</ol>
        <a href={snapshotUrl} download={`podmesh-validation-${snapshot.updatedAt}.json`} className="mt-5 inline-flex items-start gap-2 text-sm font-semibold text-accent underline underline-offset-4"><Download size={17} className="mt-0.5 shrink-0" aria-hidden="true" />{copy.download}</a>
      </section>

      <section id="validation-evidence" tabIndex={-1} className="scroll-mt-24" aria-labelledby="proofs-title">
        <h2 id="proofs-title" className="site-display text-2xl font-semibold">{copy.proofs}</h2>
        <p className="mt-3 text-sm leading-7 text-fg-muted">{copy.proofIntro}</p>
        <div className="mt-5 divide-y divide-line border-y border-line">
          {snapshot.proofs.map((proof) => (
            <details key={proof.id} id={`proof-${proof.id}`} tabIndex={-1} className="group scroll-mt-24 py-4">
              <summary className="flex cursor-pointer items-start gap-3 text-sm font-semibold"><BookOpen size={17} className="mt-0.5 shrink-0 text-accent" aria-hidden="true" /><span className="min-w-0 flex-1">{proof.title[lang]}<span className="mt-1 block text-xs font-normal text-fg-faint"><time dateTime={proof.date}>{proof.date}</time> · {proof.version}</span></span><ChevronDown size={16} className="mt-1 shrink-0 group-open:rotate-180" aria-hidden="true" /></summary>
              <p className="mt-4 pl-7 text-sm leading-7 text-fg-muted">{proof.summary[lang]}</p>
              {proof.example && <div className="mt-4 ml-7 rounded-xl border border-line bg-surface p-4">
                <p className="text-sm font-semibold">{copy.commandGuide}</p>
                <p className="mt-2 text-sm leading-6 text-fg-muted">{proof.instructions[lang]}</p>
                <pre className="mt-3 whitespace-pre-wrap break-words text-xs leading-6 [overflow-wrap:anywhere]"><code>{proof.example}</code></pre>
              </div>}
            </details>
          ))}
        </div>
      </section>

      <section className="grid gap-6 border-b border-line pb-8 sm:grid-cols-2">
        <div><h2 className="text-lg font-semibold">{copy.priorities}</h2><ol className="mt-4 list-decimal space-y-3 pl-5 text-sm leading-6 text-fg-muted">{copy.next.map((item) => <li key={item}>{item}</li>)}</ol></div>
        <div className="rounded-xl bg-surface-2 p-5"><h2 className="text-lg font-semibold">{copy.deferred}</h2><p className="mt-3 text-sm leading-7 text-fg-muted">{copy.deferredBody}</p></div>
      </section>
      <Link to="/#podmesh" className="inline-flex items-center gap-2 text-sm font-semibold text-accent">{copy.back}<ArrowUpRight size={16} aria-hidden="true" /></Link>
    </article>
  )
}
