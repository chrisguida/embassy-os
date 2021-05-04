import { Component } from '@angular/core'
import { ActivatedRoute } from '@angular/router'
import { ApiService } from 'src/app/services/api/api.service'
import { pauseFor } from 'src/app/util/misc.util'
import { BehaviorSubject } from 'rxjs'
import { copyToClipboard } from 'src/app/util/web.util'
import { AlertController, NavController, PopoverController, ToastController } from '@ionic/angular'
import { AppMetrics } from 'src/app/util/metrics.util'
import { QRComponent } from 'src/app/components/qr/qr.component'
import { AppMetricStore } from './metric-store'
import * as JSONpointer from 'json-pointer'
import { AppInstalledFull } from 'src/app/models/app-types'
import { PatchDbModel } from 'src/app/models/patch-db/patch-db-model'

@Component({
  selector: 'app-metrics',
  templateUrl: './app-metrics.page.html',
  styleUrls: ['./app-metrics.page.scss'],
})
export class AppMetricsPage {
  error = ''
  $loading$ = new BehaviorSubject(true)
  appId: string
  pointer: string
  qrCode: string
  $metrics$ = new BehaviorSubject<AppMetrics>({ })
  $hasMetrics$ = new BehaviorSubject<boolean>(null)
  unmasked: { [key: string]: boolean } = { }

  constructor (
    private readonly route: ActivatedRoute,
    private readonly apiService: ApiService,
    private readonly alertCtrl: AlertController,
    private readonly toastCtrl: ToastController,
    private readonly popoverCtrl: PopoverController,
    private readonly metricStore: AppMetricStore,
    private readonly navCtrl: NavController,
    public patch: PatchDbModel,
  ) { }

  ngOnInit () {
    this.appId = this.route.snapshot.paramMap.get('appId')
    this.pointer = this.route.queryParams['pointer']

    this.getMetrics().then(() => this.$loading$.next(false))

    this.metricStore.watch().subscribe(m => {
      const metrics = JSONpointer.get(m, this.pointer || '')
      this.$metrics$.next(metrics)
    })
    this.$metrics$.subscribe(m => {
      this.$hasMetrics$.next(!!Object.keys(m || { }).length)
    })
    this.route.queryParams.subscribe(queryParams => {
      if (queryParams['pointer'] === this.pointer) return
      this.pointer = queryParams['pointer']
      const metrics = JSONpointer.get(this.metricStore.$metrics$.getValue(), this.pointer || '')
      this.$metrics$.next(metrics)
    })
  }

  async doRefresh (event: any) {
    await this.getMetrics(),
    event.target.complete()
  }

  async presentDescription (metric: { key: string, value: AppMetrics[''] }, e: Event) {
    e.stopPropagation()

    const alert = await this.alertCtrl.create({
      header: metric.key,
      message: metric.value.description,
    })
    await alert.present()
  }

  async goToNested (key: string): Promise<any> {
    this.navCtrl.navigateForward(`/services/installed/${this.appId}/metrics`, {
      queryParams: {
        pointer: `${this.pointer || ''}/${key}/value`,
      },
    })
  }

  async copy (text: string): Promise<void> {
    let message = ''
    await copyToClipboard(text).then(success => { message = success ? 'copied to clipboard!' :  'failed to copy'})

    const toast = await this.toastCtrl.create({
      header: message,
      position: 'bottom',
      duration: 1000,
      cssClass: 'notification-toast',
    })
    await toast.present()
  }

  async showQR (text: string, ev: any): Promise<void> {
    const popover = await this.popoverCtrl.create({
      component: QRComponent,
      cssClass: 'qr-popover',
      event: ev,
      componentProps: {
        text,
      },
    })
    return await popover.present()
  }

  toggleMask (key: string) {
    this.unmasked[key] = !this.unmasked[key]
  }

  asIsOrder (a: any, b: any) {
    return 0
  }

  private async getMetrics (): Promise<void> {
    try {
      const [metrics] = await Promise.all([
        this.apiService.getAppMetrics(this.appId),
        pauseFor(600),
      ])
      this.metricStore.update(metrics)
    } catch (e) {
      console.error(e)
      this.error = e.message
    }
  }
}
