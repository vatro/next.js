import type { ExportRouteResult, FileWriter } from '../types'
import type { RenderOpts } from '../../server/app-render/types'
import type { OutgoingHttpHeaders } from 'http'
import type { NextParsedUrlQuery } from '../../server/request-meta'

import { MockedRequest, MockedResponse } from '../../server/lib/mock-request'
import { isDynamicUsageError } from '../helpers/is-dynamic-usage-error'
import { NEXT_CACHE_TAGS_HEADER } from '../../lib/constants'
import { hasNextSupport } from '../../telemetry/ci-info'
import { lazyRenderAppPage } from '../../server/future/route-modules/app-page/module.render'

export const enum ExportedAppPageFiles {
  HTML = 'HTML',
  FLIGHT = 'FLIGHT',
  META = 'META',
  POSTPONED = 'POSTPONED',
}

export async function exportAppPage(
  req: MockedRequest,
  res: MockedResponse,
  page: string,
  path: string,
  pathname: string,
  query: NextParsedUrlQuery,
  renderOpts: RenderOpts,
  htmlFilepath: string,
  debugOutput: boolean,
  isDynamicError: boolean,
  fileWriter: FileWriter
): Promise<ExportRouteResult> {
  // If the page is `/_not-found`, then we should update the page to be `/404`.
  if (page === '/_not-found') {
    pathname = '/404'
  }

  try {
    const result = await lazyRenderAppPage(
      req,
      res,
      pathname,
      query,
      renderOpts
    )
    const html = result.toUnchunkedString()
    const { metadata } = result
    const flightData = metadata.pageData
    const revalidate = metadata.revalidate

    if (revalidate === 0) {
      if (isDynamicError) {
        throw new Error(
          `Page with dynamic = "error" encountered dynamic data method on ${path}.`
        )
      }

      const { staticBailoutInfo = {} } = metadata

      if (revalidate === 0 && debugOutput && staticBailoutInfo?.description) {
        const err = new Error(
          `Static generation failed due to dynamic usage on ${path}, reason: ${staticBailoutInfo.description}`
        )

        // Update the stack if it was provided via the bailout info.
        const { stack } = staticBailoutInfo
        if (stack) {
          err.stack = err.message + stack.substring(stack.indexOf('\n'))
        }

        console.warn(err)
      }

      return { revalidate: 0 }
    }

    let headers: OutgoingHttpHeaders | undefined
    if (metadata.fetchTags) {
      headers = { [NEXT_CACHE_TAGS_HEADER]: metadata.fetchTags }
    }

    // Writing static HTML to a file.
    await fileWriter(
      ExportedAppPageFiles.HTML,
      htmlFilepath,
      html ?? '',
      'utf8'
    )

    // Writing the request metadata to a file.
    const meta = { headers }
    await fileWriter(
      ExportedAppPageFiles.META,
      htmlFilepath.replace(/\.html$/, '.meta'),
      JSON.stringify(meta)
    )

    // Writing the RSC payload to a file.
    await fileWriter(
      ExportedAppPageFiles.FLIGHT,
      htmlFilepath.replace(/\.html$/, '.rsc'),
      flightData
    )

    return {
      // Only include the metadata if the environment has next support.
      metadata: hasNextSupport ? meta : undefined,
      revalidate,
    }
  } catch (err: any) {
    if (!isDynamicUsageError(err)) {
      throw err
    }

    return { revalidate: 0 }
  }
}
